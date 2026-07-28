//! Lua scripting tools.

use super::*;

impl Agent {
    pub(super) async fn dispatch_lua(&self, name: &str, args: &Value) -> Option<ToolOutcome> {
        let outcome = match name {
            "run_lua" => {
                let script = lua_engine::strip_code_fence(str_arg(args, "script")).to_string();
                let host = Arc::new(AgentScriptHost {
                    searxng: Arc::clone(&self.searxng),
                    mcp_servers: Arc::clone(&self.mcp_servers),
                });
                let output = lua_engine::run_script(
                    script,
                    host,
                    lua_engine::LuaLimits::from_env(),
                    |s: &str| s.to_string(),
                )
                .await;
                if let Some(image) = output.image {
                    let text = if output.text.is_empty() {
                        format!(
                        "Graph rendered as PNG ({} bytes) and attached to the Discord response.",
                        image.len()
                    )
                    } else {
                        format!(
                            "{}\n\nA graph PNG image ({} bytes) was also generated and \
                         automatically attached to the Discord response.",
                            output.text,
                            image.len()
                        )
                    };
                    ToolOutcome::Attachment {
                        text,
                        attachment: AgentAttachment {
                            filename: "graph.png".to_string(),
                            bytes: image,
                        },
                    }
                } else {
                    ToolOutcome::Text(output.text)
                }
            }
            "get_lua_docs" => ToolOutcome::Text(LUA_DOCS.to_string()),
            _ => return None,
        };
        Some(outcome)
    }
}
