//! Assembling the tool list for a turn and dispatching a single tool call.

use super::*;

impl Agent {
    pub(crate) async fn build_tools(
        &self,
        deep_memory_enabled: bool,
        configurer: bool,
    ) -> Vec<Value> {
        let mut tools = Vec::new();
        for server in self.mcp_servers.iter() {
            for tool in server.list_tools().await {
                tools.push(to_openai_tool(
                    &format!("{}__{}", server.prefix, tool.name),
                    &tool.description,
                    tool.input_schema,
                ));
            }
        }
        let mut defs: Vec<Value> = vec![
            tools::searxng::definition(),
            tools::searxng::deep_research_definition(),
            tools::web_fetch::definition(),
            tools::file_download::definition(),
            tools::common_crawl::definition(),
            use_skill_tool(),
            create_skill_tool(),
            tools::manage_skills::list_definition(),
            tools::manage_skills::info_definition(),
            tools::manage_skills::delete_definition(),
            tools::manage_skills::edit_definition(),
            tools::manage_skills::enable_definition(),
            tools::manage_skills::disable_definition(),
            tools::feature_request::definition(),
            tools::edit_feature_request::definition(),
            tools::feature_development::definition(),
            tools::github_api::definition(),
            tools::remind::definition(),
            tools::summarize_url::definition(),
            tools::token_metrics::definition(),
            tools::translate::definition(),
            tools::features::definition(),
            get_messages_tool(),
            find_discord_users_tool(),
            get_discord_user_tool(),
            run_lua_tool(),
            get_lua_docs_tool(),
        ];
        defs.extend(tools::sandbox::all_definitions());
        // Configuration control is only offered to authorized configurers
        // (re-checked at dispatch as a defence-in-depth measure).
        if configurer {
            defs.push(configure_bot_tool());
        }
        // Conditionally include memory tools based on user's privacy setting.
        if deep_memory_enabled {
            defs.push(crate::memory::update_memory_tool());
            defs.push(crate::memory::search_memory_tool());
        }
        for def in defs {
            let (name, desc, params) = flatten_tool(&def);
            tools.push(to_openai_tool(&name, &desc, params));
        }
        tools
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn dispatch_tool(
        &self,
        name: &str,
        args: &Value,
        user_id: &str,
        username: &str,
        channel_id: u64,
        guild_id: Option<u64>,
        sandbox: &LazySandbox,
    ) -> ToolOutcome {
        let started = std::time::Instant::now();
        let requester_id = user_id.parse().unwrap_or(0);
        let outcome = if let Some(guild_id) = guild_id {
            match self
                .tool_permissions
                .is_banned(guild_id, requester_id, name)
                .await
            {
                Ok(true) => ToolOutcome::Text(format!(
                    "Error: permission denied — you are restricted from using `{name}` in this server."
                )),
                Ok(false) => {
                    self.dispatch_tool_inner(name, args, user_id, username, channel_id, guild_id, sandbox)
                        .await
                }
                Err(error) => {
                    tracing::error!(%error, %guild_id, "tool permission check failed");
                    ToolOutcome::Text(
                        "Error: tool permissions are temporarily unavailable; the tool call was blocked for safety."
                            .into(),
                    )
                }
            }
        } else {
            self.dispatch_tool_inner(name, args, user_id, username, channel_id, 0, sandbox)
                .await
        };
        let content = match &outcome {
            ToolOutcome::Text(t) => t.as_str(),
            ToolOutcome::Attachment { text, .. } => text.as_str(),
            ToolOutcome::DevelopmentAction { text, .. } => text.as_str(),
        };
        tracing::info!(
            target: "housebot::agent",
            user_id,
            tool = name,
            result_chars = content.chars().count(),
            is_error = content.starts_with("Error:"),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "Tool call finished"
        );
        outcome
    }
}
