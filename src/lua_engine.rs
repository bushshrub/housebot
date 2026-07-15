//! Sandboxed Lua scripting engine backing the `/lua` slash command.
//!
//! Scripts run in a restricted Lua 5.4 VM: only the `table`, `string`, and `math`
//! standard libraries are loaded, file/OS/network access is unavailable, and
//! `load`/`dofile`/`loadfile` are removed (loading untrusted bytecode is a known
//! sandbox escape in Lua 5.4). Execution is bounded by a wall-clock time limit
//! (enforced via an instruction-count hook) and a memory limit, and bridged bot
//! capabilities are capped per script run.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use mlua::{HookTriggers, Lua, LuaOptions, MultiValue, StdLib, Value as LuaValue, VmState};

use crate::agent::Agent;
use crate::config;
use crate::discord_bridge::DiscordBridge;

/// Maximum characters of captured output (print + return values) per script.
pub const MAX_OUTPUT_CHARS: usize = 4000;
/// Maximum bridged `discord.*` calls per script run.
const MAX_API_CALLS: usize = 10;
/// Maximum `discord.send_message` calls per script run.
const MAX_MESSAGES_SENT: usize = 5;
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

/// Production host: routes script capabilities through the agent and Discord bridge.
pub struct BotScriptHost {
    pub agent: Arc<Agent>,
    pub discord: Arc<DiscordBridge>,
    pub channel_id: u64,
}

#[async_trait]
impl ScriptHost for BotScriptHost {
    async fn send_message(&self, content: &str) -> Result<(), String> {
        self.discord.send_message(self.channel_id, content).await
    }

    async fn web_search(&self, query: &str, max_results: usize) -> String {
        self.agent.web_search(query, max_results).await
    }

    async fn jellyfin_search(&self, query: &str) -> String {
        self.agent.jellyfin_search(query).await
    }
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

/// Shared per-run state: captured output, bridge-call counters, and the deadline.
struct RunState {
    output: RefCell<String>,
    truncated: Cell<bool>,
    api_calls: Cell<usize>,
    messages_sent: Cell<usize>,
    deadline: Instant,
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
pub async fn run_script(script: String, host: Arc<dyn ScriptHost>, limits: LuaLimits) -> String {
    let handle = tokio::runtime::Handle::current();
    // The instruction hook cannot fire while a bridge call blocks, so give the
    // backstop some slack beyond the script's own budget before abandoning it.
    let backstop = limits.timeout * 2 + Duration::from_secs(5);
    let task = tokio::task::spawn_blocking(move || execute(&script, host, limits, handle));
    match tokio::time::timeout(backstop, task).await {
        Ok(Ok(result)) => result,
        Ok(Err(join_error)) => {
            if panicked_on_time_limit(join_error) {
                format!(
                    "Error: script exceeded the time limit ({}s).",
                    limits.timeout.as_secs()
                )
            } else {
                "Error: script execution failed unexpectedly.".to_string()
            }
        }
        Err(_) => format!(
            "Error: script exceeded the time limit ({}s).",
            limits.timeout.as_secs()
        ),
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
) -> String {
    let state = Rc::new(RunState {
        output: RefCell::new(String::new()),
        truncated: Cell::new(false),
        api_calls: Cell::new(0),
        messages_sent: Cell::new(0),
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

    let output = state.output.borrow();
    if output.trim().is_empty() {
        "(script completed with no output)".to_string()
    } else {
        output.trim_end().to_string()
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
    for name in ["dofile", "loadfile", "load"] {
        globals.raw_set(name, LuaValue::Nil)?;
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
            bridge_call(
                &jellyfin_handle,
                remaining,
                jellyfin_host.jellyfin_search(&query),
            )
        })?,
    )?;

    globals.raw_set("discord", discord)?;
    drop(globals);
    Ok(lua)
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
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeHost {
        sent: Mutex<Vec<String>>,
        searches: AtomicUsize,
    }

    #[async_trait]
    impl ScriptHost for FakeHost {
        async fn send_message(&self, content: &str) -> Result<(), String> {
            self.sent.lock().unwrap().push(content.to_string());
            Ok(())
        }

        async fn web_search(&self, query: &str, _max_results: usize) -> String {
            self.searches.fetch_add(1, Ordering::SeqCst);
            format!("results for: {query}")
        }

        async fn jellyfin_search(&self, query: &str) -> String {
            format!("media for: {query}")
        }
    }

    fn limits() -> LuaLimits {
        LuaLimits {
            timeout: Duration::from_secs(2),
            memory_bytes: 8 * 1024 * 1024,
        }
    }

    async fn run(script: &str, host: &Arc<FakeHost>, limits: LuaLimits) -> String {
        run_script(
            script.to_string(),
            Arc::clone(host) as Arc<dyn ScriptHost>,
            limits,
        )
        .await
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn returns_expression_value() {
        let host = Arc::new(FakeHost::default());
        assert_eq!(run("return 1 + 2", &host, limits()).await, "3");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn captures_print_output() {
        let host = Arc::new(FakeHost::default());
        let out = run("print(\"hello\", 42) print(\"second\")", &host, limits()).await;
        assert_eq!(out, "hello\t42\nsecond");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_output_reports_completion() {
        let host = Arc::new(FakeHost::default());
        let out = run("local x = 1", &host, limits()).await;
        assert_eq!(out, "(script completed with no output)");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sandbox_hides_dangerous_globals() {
        let host = Arc::new(FakeHost::default());
        let out = run(
            "return type(os), type(io), type(require), type(load), type(dofile), type(loadfile), type(debug), type(package)",
            &host,
            limits(),
        )
        .await;
        assert_eq!(out, "nil\tnil\tnil\tnil\tnil\tnil\tnil\tnil");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn busy_loop_hits_time_limit() {
        let host = Arc::new(FakeHost::default());
        let short = LuaLimits {
            timeout: Duration::from_millis(200),
            memory_bytes: 8 * 1024 * 1024,
        };
        let out = run("while true do end", &host, short).await;
        assert!(out.contains("time limit"), "unexpected output: {out}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pcall_cannot_swallow_the_time_limit() {
        let host = Arc::new(FakeHost::default());
        let short = LuaLimits {
            timeout: Duration::from_millis(200),
            memory_bytes: 8 * 1024 * 1024,
        };
        let started = Instant::now();
        let out = run(
            "while true do pcall(function() while true do end end) end",
            &host,
            short,
        )
        .await;
        assert!(out.contains("time limit"), "unexpected output: {out}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn error_is_visible_after_truncated_output() {
        let host = Arc::new(FakeHost::default());
        let out = run(
            "for i = 1, 100 do print(string.rep(\"a\", 100)) end error(\"boom\")",
            &host,
            limits(),
        )
        .await;
        assert!(out.contains("output truncated"), "unexpected output: {out}");
        assert!(out.contains("boom"), "unexpected output: {out}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn memory_limit_enforced() {
        let host = Arc::new(FakeHost::default());
        let small = LuaLimits {
            timeout: Duration::from_secs(5),
            memory_bytes: 1024 * 1024,
        };
        let out = run("local s = \"x\" while true do s = s .. s end", &host, small).await;
        assert!(out.contains("memory limit"), "unexpected output: {out}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn runtime_errors_are_reported() {
        let host = Arc::new(FakeHost::default());
        let out = run("error(\"boom\")", &host, limits()).await;
        assert!(out.starts_with("Error:"), "unexpected output: {out}");
        assert!(out.contains("boom"), "unexpected output: {out}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn output_before_error_is_kept() {
        let host = Arc::new(FakeHost::default());
        let out = run("print(\"before\") error(\"boom\")", &host, limits()).await;
        assert!(out.starts_with("before\n"), "unexpected output: {out}");
        assert!(out.contains("boom"), "unexpected output: {out}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn output_is_truncated() {
        let host = Arc::new(FakeHost::default());
        let out = run(
            "for i = 1, 100 do print(string.rep(\"a\", 100)) end",
            &host,
            limits(),
        )
        .await;
        assert!(out.contains("output truncated"), "unexpected output: {out}");
        assert!(out.chars().count() <= MAX_OUTPUT_CHARS + OUTPUT_TRUNCATED_MARKER.chars().count());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn web_search_bridge_works() {
        let host = Arc::new(FakeHost::default());
        let out = run("return discord.web_search(\"rust\")", &host, limits()).await;
        assert_eq!(out, "results for: rust");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn jellyfin_bridge_works() {
        let host = Arc::new(FakeHost::default());
        let out = run(
            "return discord.jellyfin_search(\"matrix\")",
            &host,
            limits(),
        )
        .await;
        assert_eq!(out, "media for: matrix");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn send_message_bridge_delivers() {
        let host = Arc::new(FakeHost::default());
        let out = run("discord.send_message(\"hi there\")", &host, limits()).await;
        assert_eq!(out, "(script completed with no output)");
        assert_eq!(*host.sent.lock().unwrap(), vec!["hi there".to_string()]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn send_message_cap_enforced() {
        let host = Arc::new(FakeHost::default());
        let out = run(
            "for i = 1, 10 do discord.send_message(\"spam\" .. i) end",
            &host,
            limits(),
        )
        .await;
        assert!(out.contains("limit"), "unexpected output: {out}");
        assert_eq!(host.sent.lock().unwrap().len(), MAX_MESSAGES_SENT);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn api_call_cap_enforced() {
        let host = Arc::new(FakeHost::default());
        let out = run(
            "for i = 1, 20 do discord.web_search(\"q\" .. i) end",
            &host,
            limits(),
        )
        .await;
        assert!(out.contains("API calls"), "unexpected output: {out}");
        assert_eq!(host.searches.load(Ordering::SeqCst), MAX_API_CALLS);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn errors_can_be_caught_with_pcall() {
        let host = Arc::new(FakeHost::default());
        let out = run(
            "local ok, err = pcall(function() error(\"inner\") end) return ok, err",
            &host,
            limits(),
        )
        .await;
        assert!(out.starts_with("false"), "unexpected output: {out}");
        assert!(out.contains("inner"), "unexpected output: {out}");
    }

    #[test]
    fn strips_fence_with_language_tag() {
        assert_eq!(strip_code_fence("```lua\nreturn 1\n```"), "return 1");
    }

    #[test]
    fn strips_bare_fence() {
        assert_eq!(strip_code_fence("```\nreturn 1\n```"), "return 1");
    }

    #[test]
    fn strips_single_line_fence() {
        assert_eq!(strip_code_fence("```return 1```"), "return 1");
    }

    #[test]
    fn strips_inline_backticks() {
        assert_eq!(strip_code_fence("`return 1`"), "return 1");
    }

    #[test]
    fn leaves_plain_script_untouched() {
        assert_eq!(strip_code_fence("  return 1  "), "return 1");
    }

    fn roles() -> Vec<(u64, String, u16)> {
        vec![
            (1, "@everyone".to_string(), 0),
            (2, "Member".to_string(), 1),
            (3, "Scripting".to_string(), 5),
            (4, "Moderator".to_string(), 7),
        ]
    }

    #[test]
    fn scripting_role_grants_access() {
        assert!(scripting_permitted(&[3], &roles(), "Scripting"));
    }

    #[test]
    fn higher_role_grants_access() {
        assert!(scripting_permitted(&[4], &roles(), "Scripting"));
    }

    #[test]
    fn lower_role_is_denied() {
        assert!(!scripting_permitted(&[1, 2], &roles(), "Scripting"));
    }

    #[test]
    fn role_name_match_is_case_insensitive() {
        assert!(scripting_permitted(&[3], &roles(), "scripting"));
    }

    #[test]
    fn missing_scripting_role_disables_feature() {
        let no_scripting = vec![
            (1, "@everyone".to_string(), 0),
            (4, "Moderator".to_string(), 7),
        ];
        assert!(!scripting_permitted(&[4], &no_scripting, "Scripting"));
    }

    #[test]
    fn no_roles_is_denied() {
        assert!(!scripting_permitted(&[], &roles(), "Scripting"));
    }
}
