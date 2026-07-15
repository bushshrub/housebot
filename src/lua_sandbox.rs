//! Sandboxed Lua execution for privileged Discord users.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mlua::{HookTriggers, Lua, LuaOptions, MultiValue, StdLib, Value, Variadic, VmState};

use crate::agent::Agent;

const MEMORY_LIMIT_BYTES: usize = 2 * 1024 * 1024;
const INSTRUCTION_LIMIT: u64 = 1_000_000;
const INSTRUCTIONS_PER_HOOK: u32 = 1_000;
const EXECUTION_TIMEOUT: Duration = Duration::from_secs(3);
const OUTPUT_LIMIT_CHARS: usize = 16_000;

#[derive(Debug, Clone, Copy)]
pub struct LuaContext {
    pub user_id: u64,
    pub channel_id: u64,
}

pub async fn execute(script: &str, context: LuaContext, agent: Arc<Agent>) -> String {
    let script = strip_code_fence(script);
    if script.trim().is_empty() {
        return "Error: Lua script cannot be empty.".to_string();
    }
    if script.chars().count() > 8_000 {
        return "Error: Lua script exceeds the 8,000-character limit.".to_string();
    }

    match tokio::time::timeout(EXECUTION_TIMEOUT, execute_inner(&script, context, agent)).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => format!("Error: Lua sandbox: {error}"),
        Err(_) => "Error: Lua sandbox execution timed out.".to_string(),
    }
}

fn strip_code_fence(script: &str) -> String {
    let trimmed = script.trim();
    let Some(after_opening) = trimmed
        .strip_prefix("```lua")
        .or_else(|| trimmed.strip_prefix("```"))
    else {
        return trimmed.to_string();
    };
    after_opening
        .strip_suffix("```")
        .unwrap_or(after_opening)
        .trim()
        .to_string()
}

async fn execute_inner(
    script: &str,
    context: LuaContext,
    agent: Arc<Agent>,
) -> mlua::Result<String> {
    let libraries = StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8;
    let lua = Lua::new_with(libraries, LuaOptions::new())?;
    lua.set_memory_limit(MEMORY_LIMIT_BYTES)?;

    let output = Arc::new(Mutex::new(Vec::<String>::new()));
    let print_output = Arc::clone(&output);
    lua.globals().set(
        "print",
        lua.create_function(move |lua, values: Variadic<Value>| {
            let line = values
                .into_iter()
                .map(|value| display_value(lua, value))
                .collect::<mlua::Result<Vec<_>>>()?
                .join("\t");
            push_output(&print_output, line);
            Ok(())
        })?,
    )?;

    let discord = lua.create_table()?;
    discord.set("user_id", context.user_id.to_string())?;
    discord.set("channel_id", context.channel_id.to_string())?;
    let message_output = Arc::clone(&output);
    discord.set(
        "send_message",
        lua.create_function(move |_lua, message: String| {
            push_output(&message_output, message);
            Ok(())
        })?,
    )?;
    let search_agent = Arc::clone(&agent);
    discord.set(
        "web_search",
        lua.create_async_function(move |_lua, query: String| {
            let agent = Arc::clone(&search_agent);
            async move {
                if query.trim().is_empty() || query.chars().count() > 300 {
                    return Ok("Error: search query must be 1-300 characters".to_string());
                }
                Ok(agent.script_web_search(&query).await)
            }
        })?,
    )?;
    let recent_agent = agent;
    discord.set(
        "recent_messages",
        lua.create_async_function(move |_lua, limit: Option<usize>| {
            let agent = Arc::clone(&recent_agent);
            let limit = limit.unwrap_or(10).clamp(1, 20);
            async move {
                Ok(agent
                    .script_recent_messages(context.channel_id, limit)
                    .await)
            }
        })?,
    )?;
    lua.globals().set("discord", discord)?;

    let function = lua
        .load(script)
        .set_name("discord-script")
        .into_function()?;
    let thread = lua.create_thread(function)?;
    let started = Instant::now();
    let instructions = Arc::new(AtomicU64::new(0));
    let hook_instructions = Arc::clone(&instructions);
    thread.set_hook(
        HookTriggers::new().every_nth_instruction(INSTRUCTIONS_PER_HOOK),
        move |_lua, _debug| {
            let total = hook_instructions
                .fetch_add(u64::from(INSTRUCTIONS_PER_HOOK), Ordering::Relaxed)
                + u64::from(INSTRUCTIONS_PER_HOOK);
            if total > INSTRUCTION_LIMIT || started.elapsed() > EXECUTION_TIMEOUT {
                return Err(mlua::Error::RuntimeError(
                    "execution limit exceeded".to_string(),
                ));
            }
            Ok(VmState::Continue)
        },
    );
    let returned = thread.into_async::<MultiValue>(()).await?;
    if !returned.is_empty() {
        let line = returned
            .into_iter()
            .map(|value| display_value(&lua, value))
            .collect::<mlua::Result<Vec<_>>>()?
            .join("\t");
        push_output(&output, line);
    }

    let lines = output
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if lines.is_empty() {
        Ok("Lua script completed with no output.".to_string())
    } else {
        Ok(lines.join("\n"))
    }
}

fn display_value(lua: &Lua, value: Value) -> mlua::Result<String> {
    match value {
        Value::Nil => Ok("nil".to_string()),
        Value::Boolean(value) => Ok(value.to_string()),
        Value::Integer(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(value.to_string()),
        Value::String(value) => Ok(value.to_string_lossy()),
        other => lua
            .coerce_string(other)?
            .map(|value| value.to_string_lossy())
            .ok_or_else(|| mlua::Error::RuntimeError("unsupported output value".to_string())),
    }
}

fn push_output(output: &Mutex<Vec<String>>, line: String) {
    let mut output = output
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let existing = output
        .iter()
        .map(|line| line.chars().count() + 1)
        .sum::<usize>();
    let remaining = OUTPUT_LIMIT_CHARS.saturating_sub(existing);
    if remaining == 0 {
        return;
    }
    output.push(line.chars().take(remaining).collect());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::History;
    use crate::memory::Memory;
    use crate::profile::ProfileStore;
    use crate::reminders::Reminders;
    use crate::skills::Skills;
    use crate::testing::MockChatClient;
    use tempfile::TempDir;

    fn test_agent() -> (TempDir, Arc<Agent>) {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let agent = Agent::for_test(
            Arc::new(MockChatClient::new()),
            History::new(root.join("history"), 30),
            Memory::new(root.join("memory")),
            ProfileStore::new(root.join("profiles")),
            Skills::new(root.join("skills.json")),
            Reminders::new(root.join("reminders.json")),
        );
        (temp, Arc::new(agent))
    }

    #[tokio::test]
    async fn captures_print_and_return_values() {
        let (_temp, agent) = test_agent();
        let output = execute(
            "print('hello', 42); return string.upper('done')",
            LuaContext {
                user_id: 1,
                channel_id: 2,
            },
            agent,
        )
        .await;
        assert_eq!(output, "hello\t42\nDONE");
    }

    #[test]
    fn accepts_discord_code_fences() {
        assert_eq!(strip_code_fence("```lua\nprint('ok')\n```"), "print('ok')");
        assert_eq!(strip_code_fence("return 1"), "return 1");
    }

    #[tokio::test]
    async fn exposes_restricted_discord_context_and_send_message() {
        let (_temp, agent) = test_agent();
        let output = execute(
            "discord.send_message(discord.user_id .. ':' .. discord.channel_id)",
            LuaContext {
                user_id: 7,
                channel_id: 9,
            },
            agent,
        )
        .await;
        assert_eq!(output, "7:9");
    }

    #[tokio::test]
    async fn blocks_host_libraries() {
        let (_temp, agent) = test_agent();
        let output = execute(
            "return os == nil and io == nil and package == nil and debug == nil",
            LuaContext {
                user_id: 1,
                channel_id: 2,
            },
            agent,
        )
        .await;
        assert_eq!(output, "true");
    }

    #[tokio::test]
    async fn stops_runaway_scripts() {
        let (_temp, agent) = test_agent();
        let output = execute(
            "while true do end",
            LuaContext {
                user_id: 1,
                channel_id: 2,
            },
            agent,
        )
        .await;
        assert!(output.contains("execution limit exceeded") || output.contains("timed out"));
    }

    #[tokio::test]
    async fn enforces_memory_limit() {
        let (_temp, agent) = test_agent();
        let output = execute(
            "return string.rep('x', 3000000)",
            LuaContext {
                user_id: 1,
                channel_id: 2,
            },
            agent,
        )
        .await;
        assert!(output.contains("memory") || output.contains("not enough"));
    }
}
