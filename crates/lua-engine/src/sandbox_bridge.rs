//! Building the sandboxed Lua VM: the `discord.*` bridge and its limits.

use crate::*;

pub(crate) fn build_sandbox(
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

pub(crate) fn clamp_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

pub(crate) fn graph_limit_error(what: &str, limit: usize) -> mlua::Error {
    mlua::Error::RuntimeError(format!("script exceeded the limit of {limit} graph {what}"))
}

/// Drive an async host call from the VM's blocking thread, bounded by the
/// script's remaining time budget.
pub(crate) fn bridge_call<T>(
    handle: &tokio::runtime::Handle,
    remaining: Duration,
    fut: impl std::future::Future<Output = T>,
) -> mlua::Result<T> {
    handle
        .block_on(async { tokio::time::timeout(remaining, fut).await })
        .map_err(|_| mlua::Error::RuntimeError(TIME_LIMIT_MARKER.to_string()))
}

pub(crate) fn format_lua_value(value: &LuaValue) -> String {
    value
        .to_string()
        .unwrap_or_else(|_| value.type_name().to_string())
}

pub(crate) fn friendly_error(error: &mlua::Error, limits: &LuaLimits) -> String {
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
