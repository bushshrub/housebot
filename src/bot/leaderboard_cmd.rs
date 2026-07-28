//! The token-leaderboard slash command.

use super::*;

impl HouseBot {
    pub(crate) async fn handle_token_leaderboard_command(
        &self,
        ctx: &Context,
        cmd: &serenity::all::CommandInteraction,
    ) {
        let user_id = cmd.user.id.get();
        let member_roles = cmd
            .member
            .as_deref()
            .map(|member| {
                member
                    .roles
                    .iter()
                    .map(|role| role.get())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let is_admin = (config::owner_id() != 0 && config::owner_id() == user_id)
            || cmd
                .member
                .as_deref()
                .and_then(|member| member.permissions)
                .is_some_and(|permissions| permissions.administrator());
        let server_config = match cmd.guild_id {
            Some(guild_id) => self.server_cfg.load(guild_id.get()).await,
            None => ServerConfig::default(),
        };
        let access = leaderboard_access(
            &server_config,
            cmd.guild_id.is_some(),
            &member_roles,
            is_admin,
        );
        let reply = if access == LeaderboardAccess::Denied {
            "This server restricts the token leaderboard to configured roles.".into()
        } else {
            let (period, metric) = leaderboard_options(&cmd.data.options);
            self.agent
                .token_leaderboard(period, metric, &user_id.to_string())
                .await
        };
        let reply = self.redactor.redact(&reply);
        let reply = truncate_memory_reply("", &reply);
        let response = CreateInteractionResponse::Message(
            CreateInteractionResponseMessage::new()
                .content(reply)
                .ephemeral(access != LeaderboardAccess::Public)
                .allowed_mentions(CreateAllowedMentions::new()),
        );
        if let Err(error) = cmd.create_response(&ctx.http, response).await {
            tracing::warn!(%error, "Failed to send /token_leaderboard response");
        }
    }
}
