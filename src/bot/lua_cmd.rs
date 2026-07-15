//! The /lua command: permission checks, safety review, and execution.

use super::*;

impl HouseBot {
    pub(crate) async fn handle_lua_command(
        &self,
        ctx: &Context,
        cmd: &serenity::all::CommandInteraction,
    ) {
        let user_id = cmd.user.id.get();
        let script = cmd
            .data
            .options
            .iter()
            .find(|o| o.name == "script")
            .and_then(|o| match &o.value {
                CommandDataOptionValue::String(s) => Some(s.clone()),
                _ => None,
            });
        let Some(script) = script else {
            respond_ephemeral(ctx, cmd, "Please provide a script to run.").await;
            return;
        };
        if !self.lua_permitted(ctx, cmd).await {
            let reply = format!(
                "You need the **{}** role (or a higher one) to run scripts.",
                lua_engine::scripting_role_name()
            );
            respond_ephemeral(ctx, cmd, &reply).await;
            return;
        }
        if self.lua_rate_limiter.check(&user_id.to_string()) {
            respond_ephemeral(
                ctx,
                cmd,
                "You're running scripts too quickly — try again in a minute.",
            )
            .await;
            return;
        }
        tracing::info!(target: "housebot::commands", user_id, "Running /lua script");
        let defer = CreateInteractionResponse::Defer(CreateInteractionResponseMessage::new());
        if let Err(e) = cmd.create_response(&ctx.http, defer).await {
            tracing::warn!("Failed to defer /lua response: {e}");
            return;
        }
        let script = lua_engine::strip_code_fence(&script).to_string();
        let _ = cmd
            .edit_response(
                &ctx.http,
                EditInteractionResponse::new()
                    .content("🔍 Reviewing the Lua script for suspicious behavior…"),
            )
            .await;
        let analysis = self.agent.analyze_lua_script(&script).await;
        if !analysis.allowed {
            let reason = self.redactor.redact(&analysis.reason);
            let reply = format!(
                "🚫 This Lua script was blocked because it was judged suspicious.\nReason: {reason}"
            );
            if let Err(e) = cmd
                .edit_response(&ctx.http, EditInteractionResponse::new().content(reply))
                .await
            {
                tracing::warn!("Failed to send blocked /lua response: {e}");
            }
            return;
        }
        let host = Arc::new(lua_engine::BotScriptHost {
            agent: Arc::clone(&self.agent),
            discord: Arc::clone(&self.discord),
            channel_id: cmd.channel_id.get(),
        });
        let redactor = Arc::clone(&self.redactor);
        let output = lua_engine::run_script(
            script,
            host,
            lua_engine::LuaLimits::from_env(),
            move |s: &str| redactor.redact(s),
        )
        .await;
        // Always set content explicitly, even when empty: omitting it on an
        // edit leaves the earlier "Reviewing…" progress message in place
        // (Discord treats an absent `content` field as "leave unchanged").
        let mut edit = EditInteractionResponse::new().content(if output.text.is_empty() {
            String::new()
        } else {
            format_lua_reply(&self.redactor.redact(&output.text))
        });
        if let Some(image) = output.image {
            edit = edit.new_attachment(CreateAttachment::bytes(image, "graph.png"));
        }
        if let Err(e) = cmd.edit_response(&ctx.http, edit).await {
            tracing::warn!("Failed to send /lua response: {e}");
        }
    }

    /// `/lua` is allowed for the bot owner, guild administrators, and members
    /// holding the scripting role or a higher one.
    pub(crate) async fn lua_permitted(
        &self,
        ctx: &Context,
        cmd: &serenity::all::CommandInteraction,
    ) -> bool {
        let user_id = cmd.user.id.get();
        let owner_id = config::owner_id();
        if owner_id != 0 && user_id == owner_id {
            return true;
        }
        let (Some(guild_id), Some(member)) = (cmd.guild_id, cmd.member.as_deref()) else {
            return false;
        };
        if member.permissions.is_some_and(|p| p.administrator()) {
            return true;
        }
        let Ok(roles) = guild_id.roles(&ctx.http).await else {
            return false;
        };
        let guild_roles: Vec<(u64, String, u16)> = roles
            .values()
            .map(|role| (role.id.get(), role.name.clone(), role.position))
            .collect();
        let member_roles: Vec<u64> = member.roles.iter().map(|r| r.get()).collect();
        lua_engine::scripting_permitted(
            &member_roles,
            &guild_roles,
            &lua_engine::scripting_role_name(),
        )
    }
}
