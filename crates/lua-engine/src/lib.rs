//! Sandboxed Lua scripting engine backing the `/lua` slash command and the
//! agent's `run_lua` tool.
//!
//! Scripts run in a restricted Lua 5.4 VM: only the `table`, `string`, and `math`
//! standard libraries are loaded, file/OS/network access is unavailable, and
//! `load`/`dofile`/`loadfile`/`require`/`collectgarbage`/`warn`/`_G`/`string.dump`
//! are removed. Execution is bounded by a wall-clock time limit (enforced via an
//! instruction-count hook) and a memory limit, bridged bot capabilities are capped
//! per script run, and bridge query arguments are length-capped to prevent large
//! payloads to external services.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use mlua::{HookTriggers, Lua, LuaOptions, MultiValue, StdLib, Value as LuaValue, VmState};

use graph_render::GraphBuilder;
use housebot_config as config;

/// Maximum characters of captured output (print + return values) per script.
pub const MAX_OUTPUT_CHARS: usize = 4000;
/// Maximum bridged `discord.*` calls per script run.
const MAX_API_CALLS: usize = 10;
/// Maximum `discord.send_message` calls per script run.
const MAX_MESSAGES_SENT: usize = 5;
/// Maximum nodes/edges a script's `graph.*` calls may add, and the character
/// cap on node ids, labels, and the graph title.
const MAX_GRAPH_NODES: usize = 16;
const MAX_GRAPH_EDGES: usize = 32;
const MAX_GRAPH_TEXT_CHARS: usize = 60;
/// Maximum characters accepted for a bridge search query (web_search / jellyfin_search).
const MAX_QUERY_CHARS: usize = 500;
/// How often (in VM instructions) the time-limit hook fires.
const HOOK_INSTRUCTION_INTERVAL: u32 = 4096;
/// Marker embedded in the hook error so it can be recognized after Lua wraps it.
const TIME_LIMIT_MARKER: &str = "script exceeded the time limit";
const OUTPUT_TRUNCATED_MARKER: &str = "\n… (output truncated)";

/// Bot capabilities exposed to Lua scripts through the `discord` table.
#[async_trait]
pub trait ScriptHost: Send + Sync {
    /// Send a message to the channel the script was invoked from.
    async fn send_message(&self, content: &str) -> Result<(), String>;
    /// Search the web; returns formatted results or an `Error: …` string.
    async fn web_search(&self, query: &str, max_results: usize) -> String;
    /// Search the household Jellyfin media server.
    async fn jellyfin_search(&self, query: &str) -> String;
}

/// Execution limits, resolved from `LUA_TIMEOUT_SECS` and `LUA_MEMORY_LIMIT_MB`.
#[derive(Clone, Copy)]
pub struct LuaLimits {
    pub timeout: Duration,
    pub memory_bytes: usize,
}

impl LuaLimits {
    pub fn from_env() -> Self {
        let timeout_secs = config::env_parse("LUA_TIMEOUT_SECS", 5u64).clamp(1, 30);
        let memory_mb = config::env_parse("LUA_MEMORY_LIMIT_MB", 16usize).clamp(1, 256);
        Self {
            timeout: Duration::from_secs(timeout_secs),
            memory_bytes: memory_mb * 1024 * 1024,
        }
    }
}

/// Shared per-run state: captured output, bridge-call counters, the deadline,
/// and any graph the script has built via `graph.node`/`graph.edge`.
struct RunState {
    output: RefCell<String>,
    truncated: Cell<bool>,
    api_calls: Cell<usize>,
    messages_sent: Cell<usize>,
    graph: RefCell<GraphBuilder>,
    deadline: Instant,
}

/// A script's captured text output plus an optional rendered graph image.
pub struct ScriptOutput {
    pub text: String,
    pub image: Option<Vec<u8>>,
}

impl RunState {
    fn push_output(&self, text: &str) {
        if self.truncated.get() {
            return;
        }
        let mut output = self.output.borrow_mut();
        let remaining = MAX_OUTPUT_CHARS.saturating_sub(output.chars().count());
        if text.chars().count() > remaining {
            output.extend(text.chars().take(remaining));
            output.push_str(OUTPUT_TRUNCATED_MARKER);
            self.truncated.set(true);
        } else {
            output.push_str(text);
        }
    }

    /// Account for one bridged call; errors once a cap or the deadline is hit.
    fn take_api_slot(&self) -> mlua::Result<Duration> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(mlua::Error::RuntimeError(TIME_LIMIT_MARKER.to_string()));
        }
        if self.api_calls.get() >= MAX_API_CALLS {
            return Err(mlua::Error::RuntimeError(format!(
                "script exceeded the limit of {MAX_API_CALLS} discord API calls"
            )));
        }
        self.api_calls.set(self.api_calls.get() + 1);
        Ok(remaining)
    }
}

/// Strip a surrounding markdown code fence (```lua … ``` or ``` … ```) or inline
/// backticks from a submitted script.
pub fn strip_code_fence(script: &str) -> &str {
    let script = script.trim();
    if let Some(inner) = script
        .strip_prefix("```")
        .and_then(|s| s.strip_suffix("```"))
    {
        // Drop a language tag such as `lua` on the opening fence line.
        let inner = match inner.split_once('\n') {
            Some((first, rest))
                if !first.is_empty() && first.chars().all(|c| c.is_ascii_alphanumeric()) =>
            {
                rest
            }
            _ => inner,
        };
        return inner.trim();
    }
    if script.len() > 2 {
        if let Some(inner) = script.strip_prefix('`').and_then(|s| s.strip_suffix('`')) {
            return inner.trim();
        }
    }
    script
}

/// Decide whether a member may run scripts: they need the scripting role or any
/// role positioned at or above it in the guild's role hierarchy.
///
/// `guild_roles` holds `(role_id, name, position)` for every role in the guild.
/// A guild without a role named `scripting_role_name` has scripting disabled.
pub fn scripting_permitted(
    member_role_ids: &[u64],
    guild_roles: &[(u64, String, u16)],
    scripting_role_name: &str,
) -> bool {
    let Some(required_position) = guild_roles
        .iter()
        .find(|(_, name, _)| name.eq_ignore_ascii_case(scripting_role_name))
        .map(|(_, _, position)| *position)
    else {
        return false;
    };
    member_role_ids.iter().any(|member_role| {
        guild_roles
            .iter()
            .any(|(id, _, position)| id == member_role && *position >= required_position)
    })
}

/// The configured name of the role that grants scripting access.
pub fn scripting_role_name() -> String {
    config::env_or("SCRIPTING_ROLE_NAME", "Scripting")
}

/// Run a script in the sandbox and return its captured output or an error message.
///
/// The VM runs on a blocking thread; bridged `discord.*` calls are driven on the
/// async runtime with the script's remaining time budget as their timeout.
pub async fn run_script(
    script: String,
    host: Arc<dyn ScriptHost>,
    limits: LuaLimits,
    redact: impl Fn(&str) -> String + Send + 'static,
) -> ScriptOutput {
    let handle = tokio::runtime::Handle::current();
    // The instruction hook cannot fire while a bridge call blocks, so give the
    // backstop some slack beyond the script's own budget before abandoning it.
    let backstop = limits.timeout * 2 + Duration::from_secs(5);
    let task = tokio::task::spawn_blocking(move || execute(&script, host, limits, handle, &redact));
    let timeout_output = || ScriptOutput {
        text: format!(
            "Error: script exceeded the time limit ({}s).",
            limits.timeout.as_secs()
        ),
        image: None,
    };
    match tokio::time::timeout(backstop, task).await {
        Ok(Ok(result)) => result,
        Ok(Err(join_error)) => {
            if panicked_on_time_limit(join_error) {
                timeout_output()
            } else {
                ScriptOutput {
                    text: "Error: script execution failed unexpectedly.".to_string(),
                    image: None,
                }
            }
        }
        Err(_) => timeout_output(),
    }
}

/// Whether the execution thread died from the hook's time-limit panic (used to
/// hard-kill scripts that swallow the timeout error with pcall).
fn panicked_on_time_limit(join_error: tokio::task::JoinError) -> bool {
    let Ok(payload) = join_error.try_into_panic() else {
        return false;
    };
    payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .is_some_and(|message| message.contains(TIME_LIMIT_MARKER))
}

fn execute(
    script: &str,
    host: Arc<dyn ScriptHost>,
    limits: LuaLimits,
    handle: tokio::runtime::Handle,
    redact: &dyn Fn(&str) -> String,
) -> ScriptOutput {
    let state = Rc::new(RunState {
        output: RefCell::new(String::new()),
        truncated: Cell::new(false),
        api_calls: Cell::new(0),
        messages_sent: Cell::new(0),
        graph: RefCell::new(GraphBuilder::default()),
        deadline: Instant::now() + limits.timeout,
    });
    // The returned values borrow from the VM, so keep `lua` alive until they
    // are rendered into the output buffer.
    match build_sandbox(host, handle, &state, limits) {
        Ok(lua) => match lua.load(script).set_name("script").eval::<MultiValue>() {
            Ok(values) => {
                if !values.is_empty() {
                    let rendered: Vec<String> = values.iter().map(format_lua_value).collect();
                    state.push_output(&rendered.join("\t"));
                }
            }
            Err(e) => report_error(&state, &e, &limits),
        },
        Err(e) => report_error(&state, &e, &limits),
    }

    let image = render_graph(&state, redact);

    let output = state.output.borrow();
    let trimmed = output.trim_end();
    let text = if trimmed.is_empty() {
        if image.is_some() {
            String::new()
        } else {
            "(script completed with no output)".to_string()
        }
    } else {
        trimmed.to_string()
    };
    ScriptOutput { text, image }
}

/// Render the script's graph, if it built one. Node/edge labels and the
/// title are script-supplied text that may echo back a `discord.web_search`
/// or `discord.jellyfin_search` result, so they're redacted before
/// rendering — pixels can't be redacted after the fact the way `output.text`
/// is at the call site. Render failures are appended to the output as an
/// error line, past the truncation cap, same as `report_error`.
fn render_graph(state: &RunState, redact: &dyn Fn(&str) -> String) -> Option<Vec<u8>> {
    if state.graph.borrow().is_empty() {
        return None;
    }
    state.graph.borrow_mut().redact_with(redact);
    match graph_render::render_png(&state.graph.borrow()) {
        Ok(bytes) => Some(bytes),
        Err(e) => {
            let mut output = state.output.borrow_mut();
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str(&format!("Error: failed to render graph: {e}"));
            None
        }
    }
}

/// Append a friendly error message to the output, past any truncation cap so
/// the failure is always visible. Error messages are bounded by
/// `friendly_error`, so the direct append cannot grow without limit.
fn report_error(state: &RunState, error: &mlua::Error, limits: &LuaLimits) {
    let message = friendly_error(error, limits);
    let mut output = state.output.borrow_mut();
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
    output.push_str(&message);
}

mod sandbox_bridge;

pub(crate) use sandbox_bridge::*;

#[cfg(test)]
#[path = "tests_bridge.rs"]
mod tests_bridge;
#[cfg(test)]
#[path = "tests_scripting.rs"]
mod tests_scripting;
#[cfg(test)]
#[path = "tests_support.rs"]
mod tests_support;
