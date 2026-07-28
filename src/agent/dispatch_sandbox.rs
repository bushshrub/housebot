//! Sandbox tools and the MCP-prefixed fallthrough.

use super::*;

impl Agent {
    pub(super) async fn dispatch_sandbox_or_mcp(
        &self,
        name: &str,
        args: &Value,
        sandbox: &LazySandbox,
    ) -> ToolOutcome {
        match name {
            name if name.starts_with("sandbox_") => match name {
                "sandbox_clone_repository" => ToolOutcome::Text(
                    sandbox
                        .clone_repository(
                            str_arg(args, "url"),
                            args.get("branch").and_then(Value::as_str),
                        )
                        .await
                        .unwrap_or_else(|e| format!("Error: {e}")),
                ),
                "sandbox_list_files" => ToolOutcome::Text(
                    sandbox
                        .list_files(
                            str_arg(args, "path"),
                            args.get("max_depth")
                                .and_then(Value::as_u64)
                                .map(|d| d as u32),
                        )
                        .await
                        .unwrap_or_else(|e| format!("Error: {e}")),
                ),
                "sandbox_search_code" => ToolOutcome::Text(
                    sandbox
                        .search_code(
                            str_arg(args, "query"),
                            args.get("path").and_then(Value::as_str),
                            args.get("glob").and_then(Value::as_str),
                        )
                        .await
                        .unwrap_or_else(|e| format!("Error: {e}")),
                ),
                "sandbox_read_file" => ToolOutcome::Text(
                    sandbox
                        .read_file(
                            str_arg(args, "path"),
                            args.get("start_line")
                                .and_then(Value::as_u64)
                                .map(|l| l as u32),
                            args.get("end_line")
                                .and_then(Value::as_u64)
                                .map(|l| l as u32),
                        )
                        .await
                        .unwrap_or_else(|e| format!("Error: {e}")),
                ),
                "sandbox_run" => ToolOutcome::Text(
                    sandbox
                        .run(
                            str_arg(args, "command"),
                            args.get("working_dir").and_then(Value::as_str),
                            args.get("timeout").and_then(Value::as_u64),
                        )
                        .await
                        .unwrap_or_else(|e| format!("Error: {e}")),
                ),
                _ => ToolOutcome::Text(format!("Unknown tool: {name}")),
            },
            _ if name.contains("__") => {
                let (prefix, tool_name) = name.split_once("__").unwrap();
                for server in self.mcp_servers.iter() {
                    if server.prefix == prefix {
                        return match server.call_tool(tool_name, args.clone()).await {
                            Ok(text) => ToolOutcome::Text(text),
                            Err(e) => ToolOutcome::Text(format!("Error: {e}")),
                        };
                    }
                }
                ToolOutcome::Text(format!("Unknown tool: {name}"))
            }
            _ => ToolOutcome::Text(format!("Unknown tool: {name}")),
        }
    }
}
