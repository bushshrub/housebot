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

fn build_sandbox(
    host: Arc<dyn ScriptHost>,
    handle: tokio::runtime::Handle,
    state: &Rc<RunState>,
    limits: LuaLimits,
) -> mlua::Result<Lua> {
    // `catch_rust_panics(false)` replaces pcall/xpcall with variants that
    // resume Rust panics instead of catching them — required for the hook's
    // panic escalation below to reliably end runaway scripts.
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH,
        LuaOptions::new().catch_rust_panics(false),
    )?;
    lua.set_memory_limit(limits.memory_bytes)?;

    let deadline = state.deadline;
    let timed_out = Cell::new(false);
    lua.set_global_hook(
        HookTriggers::new().every_nth_instruction(HOOK_INSTRUCTION_INTERVAL),
        move |_, _| {
            if Instant::now() >= deadline {
                // A second firing past the deadline means the script swallowed
                // the first timeout error with pcall. Panic instead: mlua's
                // pcall/xpcall cannot catch Rust panics, so this reliably ends
                // the run (surfaced as a JoinError in `run_script`).
                if timed_out.replace(true) {
                    panic!("{TIME_LIMIT_MARKER}");
                }
                return Err(mlua::Error::RuntimeError(TIME_LIMIT_MARKER.to_string()));
            }
            Ok(VmState::Continue)
        },
    )?;

    let globals = lua.globals();
    // The base library is always loaded; remove the pieces that reach outside
    // the sandbox or load untrusted chunks.
    //
    // `require` is included even though it cannot succeed without the `package`
    // library (which is not loaded): belt-and-suspenders removal.
    //
    // `collectgarbage` is removed because pausing or manipulating the GC can
    // confuse the per-allocation memory-limit callback and waste worker-thread
    // time in tight GC-manipulation loops.
    //
    // `warn` (Lua 5.4) writes to stderr bypassing sandbox output capture.
    //
    // `_G` is the explicit reference to the global table; removing it prevents
    // scripts from enumerating or bulk-modifying globals via table iteration.
    for name in [
        "dofile",
        "loadfile",
        "load",
        "require",
        "collectgarbage",
        "warn",
        "_G",
    ] {
        globals.raw_set(name, LuaValue::Nil)?;
    }

    // Pattern matching (find/match/gmatch/gsub) runs entirely inside a single C
    // call, during which the instruction hook never fires — a crafted pattern
    // (polynomial backtracking over a long subject) would run past the timeout,
    // and `spawn_blocking` cannot be cancelled, so it would pin a worker thread.
    // Remove them; the remaining string functions are linear and memory-bounded.
    // Nil-ing them on the `string` table also disables the `("x"):find(…)` method
    // form, since the string metatable's `__index` is this table.
    //
    // `string.dump` serialises a Lua function to raw bytecode. It cannot be
    // loaded back (since `load` is nil'd), but leaving it available would let a
    // script extract and exfiltrate function bytecode. Remove it.
    let string_lib: mlua::Table = globals.get("string")?;
    for name in ["find", "match", "gmatch", "gsub", "dump"] {
        string_lib.raw_set(name, LuaValue::Nil)?;
    }

    let print_state = Rc::clone(state);
    globals.raw_set(
        "print",
        lua.create_function(move |_, args: MultiValue| {
            let rendered: Vec<String> = args.iter().map(format_lua_value).collect();
            print_state.push_output(&rendered.join("\t"));
            print_state.push_output("\n");
            Ok(())
        })?,
    )?;

    let discord = lua.create_table()?;

    let send_state = Rc::clone(state);
    let send_host = Arc::clone(&host);
    let send_handle = handle.clone();
    discord.raw_set(
        "send_message",
        lua.create_function(move |_, content: String| {
            if send_state.messages_sent.get() >= MAX_MESSAGES_SENT {
                return Err(mlua::Error::RuntimeError(format!(
                    "script exceeded the limit of {MAX_MESSAGES_SENT} sent messages"
                )));
            }
            let remaining = send_state.take_api_slot()?;
            send_state
                .messages_sent
                .set(send_state.messages_sent.get() + 1);
            let content: String = content.chars().take(2000).collect();
            bridge_call(&send_handle, remaining, send_host.send_message(&content))?
                .map_err(mlua::Error::RuntimeError)
        })?,
    )?;

    let search_state = Rc::clone(state);
    let search_host = Arc::clone(&host);
    let search_handle = handle.clone();
    discord.raw_set(
        "web_search",
        lua.create_function(move |_, (query, max_results): (String, Option<usize>)| {
            let remaining = search_state.take_api_slot()?;
            let max_results = max_results.unwrap_or(10).clamp(1, 20);
            let query: String = query.chars().take(MAX_QUERY_CHARS).collect();
            bridge_call(
                &search_handle,
                remaining,
                search_host.web_search(&query, max_results),
            )
        })?,
    )?;

    let jellyfin_state = Rc::clone(state);
    let jellyfin_host = Arc::clone(&host);
    let jellyfin_handle = handle.clone();
    discord.raw_set(
        "jellyfin_search",
        lua.create_function(move |_, query: String| {
            let remaining = jellyfin_state.take_api_slot()?;
            let query: String = query.chars().take(MAX_QUERY_CHARS).collect();
            bridge_call(
                &jellyfin_handle,
                remaining,
                jellyfin_host.jellyfin_search(&query),
            )
        })?,
    )?;

    globals.raw_set("discord", discord)?;

    let graph = lua.create_table()?;

    let node_state = Rc::clone(state);
    graph.raw_set(
        "node",
        lua.create_function(move |_, (id, label): (String, Option<String>)| {
            let id = clamp_chars(&id, MAX_GRAPH_TEXT_CHARS);
            let mut builder = node_state.graph.borrow_mut();
            if !builder.has_node(&id) && builder.node_count() >= MAX_GRAPH_NODES {
                return Err(graph_limit_error("nodes", MAX_GRAPH_NODES));
            }
            let label = clamp_chars(&label.unwrap_or_else(|| id.clone()), MAX_GRAPH_TEXT_CHARS);
            builder.add_node(&id, &label);
            Ok(())
        })?,
    )?;

    let edge_state = Rc::clone(state);
    graph.raw_set(
        "edge",
        lua.create_function(move |_, (from, to): (String, String)| {
            let from = clamp_chars(&from, MAX_GRAPH_TEXT_CHARS);
            let to = clamp_chars(&to, MAX_GRAPH_TEXT_CHARS);
            let mut builder = edge_state.graph.borrow_mut();
            if builder.edge_count() >= MAX_GRAPH_EDGES {
                return Err(graph_limit_error("edges", MAX_GRAPH_EDGES));
            }
            // Checked and created one endpoint at a time: `to` may be the
            // node that fills the last slot `from` just took, so its check
            // must see the count *after* `from` was (maybe) created.
            if !builder.has_node(&from) && builder.node_count() >= MAX_GRAPH_NODES {
                return Err(graph_limit_error("nodes", MAX_GRAPH_NODES));
            }
            let from_i = builder.get_or_create(&from);
            if !builder.has_node(&to) && builder.node_count() >= MAX_GRAPH_NODES {
                return Err(graph_limit_error("nodes", MAX_GRAPH_NODES));
            }
            let to_i = builder.get_or_create(&to);
            builder.add_edge(from_i, to_i);
            Ok(())
        })?,
    )?;

    let title_state = Rc::clone(state);
    graph.raw_set(
        "title",
        lua.create_function(move |_, title: String| {
            title_state
                .graph
                .borrow_mut()
                .set_title(&clamp_chars(&title, MAX_GRAPH_TEXT_CHARS));
            Ok(())
        })?,
    )?;

    globals.raw_set("graph", graph)?;
    drop(globals);
    Ok(lua)
}

fn clamp_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn graph_limit_error(what: &str, limit: usize) -> mlua::Error {
    mlua::Error::RuntimeError(format!("script exceeded the limit of {limit} graph {what}"))
}

/// Drive an async host call from the VM's blocking thread, bounded by the
/// script's remaining time budget.
fn bridge_call<T>(
    handle: &tokio::runtime::Handle,
    remaining: Duration,
    fut: impl std::future::Future<Output = T>,
) -> mlua::Result<T> {
    handle
        .block_on(async { tokio::time::timeout(remaining, fut).await })
        .map_err(|_| mlua::Error::RuntimeError(TIME_LIMIT_MARKER.to_string()))
}

fn format_lua_value(value: &LuaValue) -> String {
    value
        .to_string()
        .unwrap_or_else(|_| value.type_name().to_string())
}

fn friendly_error(error: &mlua::Error, limits: &LuaLimits) -> String {
    let text = error.to_string();
    if text.contains(TIME_LIMIT_MARKER) {
        return format!(
            "Error: script exceeded the time limit ({}s).",
            limits.timeout.as_secs()
        );
    }
    if matches!(error, mlua::Error::MemoryError(_)) || text.contains("not enough memory") {
        return format!(
            "Error: script exceeded the memory limit ({} MB).",
            limits.memory_bytes / (1024 * 1024)
        );
    }
    let mut message = text.lines().take(4).collect::<Vec<_>>().join("\n");
    if message.chars().count() > 500 {
        message = message.chars().take(500).collect();
    }
    format!("Error: {message}")
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
