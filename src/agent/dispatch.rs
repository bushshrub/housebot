//! Tool dispatch: routes a tool call to the module that implements it.
//!
//! Each `dispatch_*` helper returns `None` for names it does not own, so the
//! order of the chain preserves the original match order — literal names first,
//! then the `sandbox_` prefix and the `__`-qualified MCP fallthrough.

use super::*;

impl Agent {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn dispatch_tool_inner(
        &self,
        name: &str,
        args: &Value,
        user_id: &str,
        username: &str,
        channel_id: u64,
        guild_id: u64,
        sandbox: &LazySandbox,
    ) -> ToolOutcome {
        if let Some(outcome) = self.dispatch_web(name, args).await {
            return outcome;
        }
        if let Some(outcome) = self
            .dispatch_features(name, args, user_id, username, channel_id, guild_id)
            .await
        {
            return outcome;
        }
        if let Some(outcome) = self.dispatch_skills(name, args, user_id).await {
            return outcome;
        }
        if let Some(outcome) = self.dispatch_discord(name, args, user_id, channel_id).await {
            return outcome;
        }
        if let Some(outcome) = self.dispatch_lua(name, args).await {
            return outcome;
        }
        if let Some(outcome) = self.dispatch_configure_bot(name, args, user_id).await {
            return outcome;
        }
        self.dispatch_sandbox_or_mcp(name, args, sandbox).await
    }
}
