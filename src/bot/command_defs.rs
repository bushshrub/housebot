//! Slash-command definitions and registration.

use super::command_defs_config::*;
use super::command_defs_moderation::*;
use super::command_defs_user::*;
use super::*;

pub(crate) fn session_command_definition() -> CreateCommand {
    CreateCommand::new("session")
        .description("View or manage your current conversation session")
        .add_option(CreateCommandOption::new(
            CommandOptionType::SubCommand,
            "status",
            "Show context and token usage for this session",
        ))
        .add_option(CreateCommandOption::new(
            CommandOptionType::SubCommand,
            "new",
            "Clear the current conversation and start fresh",
        ))
        .add_option(CreateCommandOption::new(
            CommandOptionType::SubCommand,
            "compact",
            "Summarize the conversation and start fresh",
        ))
}

pub(crate) fn storage_command_definition() -> CreateCommand {
    CreateCommand::new("storage")
        .description("Manage persistent memories and personal notes")
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::SubCommandGroup,
                "memory",
                "Manage facts the bot remembers across conversations",
            )
            .add_sub_option(CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "show",
                "Show what the bot remembers about you",
            ))
            .add_sub_option(
                CreateCommandOption::new(
                    CommandOptionType::SubCommand,
                    "search",
                    "Search your persistent memories",
                )
                .add_sub_option(
                    CreateCommandOption::new(
                        CommandOptionType::String,
                        "query",
                        "Keyword or phrase to find",
                    )
                    .required(true),
                ),
            )
            .add_sub_option(CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "clear",
                "Clear everything the bot remembers about you",
            )),
        )
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::SubCommandGroup,
                "notes",
                "Manage your named personal notes",
            )
            .add_sub_option(CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "list",
                "List your saved notes",
            ))
            .add_sub_option(
                CreateCommandOption::new(CommandOptionType::SubCommand, "get", "Read a saved note")
                    .add_sub_option(
                        CreateCommandOption::new(CommandOptionType::String, "name", "Note name")
                            .required(true),
                    ),
            )
            .add_sub_option(
                CreateCommandOption::new(
                    CommandOptionType::SubCommand,
                    "save",
                    "Create or replace a saved note",
                )
                .add_sub_option(
                    CreateCommandOption::new(CommandOptionType::String, "name", "Note name")
                        .required(true),
                )
                .add_sub_option(
                    CreateCommandOption::new(CommandOptionType::String, "content", "Text to save")
                        .required(true),
                ),
            )
            .add_sub_option(
                CreateCommandOption::new(
                    CommandOptionType::SubCommand,
                    "delete",
                    "Delete a saved note",
                )
                .add_sub_option(
                    CreateCommandOption::new(CommandOptionType::String, "name", "Note name")
                        .required(true),
                ),
            ),
        )
}

pub(crate) fn skill_command_definition() -> CreateCommand {
    CreateCommand::new("skill")
        .description("Manage custom prompt skills shared across all users")
        .add_option(CreateCommandOption::new(
            CommandOptionType::SubCommand,
            "list",
            "List all available skills",
        ))
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "info",
                "Show a skill's prompt",
            )
            .add_sub_option(
                CreateCommandOption::new(CommandOptionType::String, "name", "Skill name")
                    .required(true),
            ),
        )
        .add_option(
            CreateCommandOption::new(CommandOptionType::SubCommand, "delete", "Delete a skill")
                .add_sub_option(
                    CreateCommandOption::new(CommandOptionType::String, "name", "Skill name")
                        .required(true),
                ),
        )
}

pub(crate) fn data_command_definition() -> CreateCommand {
    CreateCommand::new("data")
        .description("Inspect or delete data associated with your account")
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::SubCommandGroup,
                "profile",
                "Inspect or clear learned profile data",
            )
            .add_sub_option(CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "show",
                "Show your stored profile information",
            ))
            .add_sub_option(CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "clear",
                "Clear learned profile data and memory",
            )),
        )
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::SubCommandGroup,
                "history",
                "Inspect or clear conversation history",
            )
            .add_sub_option(CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "show",
                "Show recent conversation history",
            ))
            .add_sub_option(CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "clear",
                "Clear your conversation history",
            )),
        )
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "erase",
                "Permanently erase all stored data and token statistics",
            )
            .add_sub_option(
                CreateCommandOption::new(
                    CommandOptionType::Boolean,
                    "confirm",
                    "Confirm permanent deletion",
                )
                .required(true),
            ),
        )
}

/// Build the `/effort` command, including administrator user targeting.
pub(crate) fn effort_command_definition() -> CreateCommand {
    let mut level = CreateCommandOption::new(
        CommandOptionType::String,
        "level",
        "Thinking effort level (omit to show the current setting)",
    );
    for mode in ThinkingMode::ALL {
        level = level.add_string_choice(format!("{mode} ({})", mode.budget_label()), mode.as_str());
    }
    CreateCommand::new("effort")
        .description("Set how much thinking the model does before replying")
        .add_option(level)
        .add_option(CreateCommandOption::new(
            CommandOptionType::User,
            "user",
            "User to configure (server administrators and bot configurers only)",
        ))
}

pub(crate) async fn register_slash_commands(ctx: &Context, guild_ids: &[GuildId]) {
    // Stable commands are global-only: a global command is visible in every
    // guild *and* in DMs, which most of the bot's commands rely on (e.g.
    // /config and /effort are meant to work from a DM with the bot). Global
    // registration can take up to an hour to propagate, which is fine for
    // commands that aren't changing.
    //
    // guild_only_commands are commands that only make sense inside a guild
    // (they already refuse to run from a DM) — registering them per guild
    // instead of globally means they never need to appear in DMs at all.
    //
    // /labs is the one deliberate exception: any new command in active
    // development ships as a /labs subcommand first (see AGENTS.md), so it is
    // registered in *both* scopes — per guild for instant availability while
    // iterating, and globally (slower to propagate) so it still eventually
    // reaches DMs. This means /labs itself always appears twice in a guild's
    // command picker; every other command stays in exactly one scope.
    let mut global_commands: Vec<CreateCommand> = Vec::new();
    let mut guild_only_commands: Vec<CreateCommand> = Vec::new();
    // The /config global slash command (bot configuration, configurers only).

    global_commands.push(config_command_definition());
    // The /server-config slash command (server administrators and bot
    // configurers). Guild-only: it refuses to run from a DM anyway.
    guild_only_commands.push(server_config_command_definition());
    global_commands.push(personalize_command_definition());
    // /labs: every new experimental feature lands here first (see AGENTS.md),
    // then gets promoted into its own command once stable. Registered in both
    // scopes below — see the comment at the top of this function.
    let labs_cmd = labs_command_definition();
    global_commands.push(effort_command_definition());
    guild_only_commands.push(tool_ban_command_definition());
    guild_only_commands.push(tool_restore_command_definition());
    let lua_cmd = lua_command_definition();
    global_commands.push(lua_cmd.clone());
    let guild_id = match std::env::var("DEPLOYMENT_GUILD_ID") {
        Ok(value) => match value.parse::<u64>() {
            Ok(id) if id != 0 => Some(id),
            Ok(_) => {
                tracing::warn!("DEPLOYMENT_GUILD_ID is set to 0, ignoring");
                None
            }
            Err(_) => {
                tracing::warn!(
                    "DEPLOYMENT_GUILD_ID is set but invalid (must be a valid u64): {}",
                    value
                );
                None
            }
        },
        Err(_) => None,
    };
    // Only needed when the bot is not a member of the deployment guild;
    // member guilds get the full command set (including /lua) below.
    if let Some(guild_id) = guild_id.filter(|id| !guild_ids.contains(&GuildId::new(*id))) {
        if let Err(e) = GuildId::new(guild_id)
            .create_command(&ctx.http, lua_cmd)
            .await
        {
            tracing::error!(
                guild_id,
                "Failed to register /lua slash command to guild: {e}"
            );
        } else {
            tracing::info!(guild_id, "Registered /lua slash command to guild");
        }
    }

    global_commands.extend([
        CreateCommand::new("help").description("Show all available commands"),
        CreateCommand::new("commit").description("Show the bot's running commit hash"),
        CreateCommand::new("model").description("Show information about the current model"),
        session_command_definition(),
        CreateCommand::new("token_leaderboard")
            .description("Show token usage rankings")
            .add_option(
                CreateCommandOption::new(
                    CommandOptionType::String,
                    "timeframe",
                    "Ranking timeframe (default: all time)",
                )
                .add_string_choice("Daily", "daily")
                .add_string_choice("Weekly", "weekly")
                .add_string_choice("Monthly", "monthly")
                .add_string_choice("All time", "all_time"),
            )
            .add_option(
                CreateCommandOption::new(
                    CommandOptionType::String,
                    "metric",
                    "Ranking metric (default: total tokens)",
                )
                .add_string_choice("Total tokens", "tokens")
                .add_string_choice("Cache efficiency", "efficiency"),
            ),
        CreateCommand::new("status")
            .description("Show your current settings (effort level, follow-up, personality)"),
        skill_command_definition(),
        CreateCommand::new("stats").description("Show your conversation and memory statistics"),
        data_command_definition(),
        CreateCommand::new("privacy")
            .description("View or change your privacy settings")
            .add_option(CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "status",
                "Show current privacy settings",
            ))
            .add_option(
                CreateCommandOption::new(
                    CommandOptionType::SubCommand,
                    "deep_memory",
                    "Toggle deep memory",
                )
                .add_sub_option(
                    CreateCommandOption::new(
                        CommandOptionType::Boolean,
                        "enabled",
                        "Enable or disable deep memory",
                    )
                    .required(true),
                ),
            ),
        storage_command_definition(),
    ]);

    // Global scope: stable commands plus /labs (slower to reach DMs, but it
    // gets there eventually). A plain per-command upsert, not a bulk
    // overwrite, so a transient failure here can't wipe out commands that
    // registered fine; the retired-command sweep below cleans up stale names.
    for command in global_commands.iter().cloned().chain([labs_cmd.clone()]) {
        if let Err(e) = Command::create_global_command(&ctx.http, command).await {
            tracing::error!("Failed to register slash command: {e}");
        }
    }

    // Guild scope: only commands that don't belong globally (guild_only_commands
    // already refuse to run from a DM) plus /labs, so /labs updates instantly
    // while a new feature is under active iteration. Bulk overwrite so stale
    // guild commands from a previous run are atomically replaced.
    let mut per_guild_commands = guild_only_commands;
    per_guild_commands.push(labs_cmd);
    for guild_id in guild_ids {
        match guild_id
            .set_commands(&ctx.http, per_guild_commands.clone())
            .await
        {
            Ok(registered) => tracing::info!(
                guild_id = guild_id.get(),
                commands = registered.len(),
                "Reinitialized guild slash commands"
            ),
            Err(error) => tracing::error!(
                guild_id = guild_id.get(),
                %error,
                "Failed to reinitialize guild slash commands"
            ),
        }
    }

    match Command::get_global_commands(&ctx.http).await {
        Ok(commands) => {
            for command in commands {
                if RETIRED_SLASH_COMMANDS.contains(&command.name.as_str()) {
                    if let Err(error) = Command::delete_global_command(&ctx.http, command.id).await
                    {
                        tracing::warn!(
                            command = %command.name,
                            %error,
                            "Failed to remove retired slash command"
                        );
                    }
                }
            }
        }
        Err(error) => tracing::warn!(%error, "Failed to inspect retired slash commands"),
    }
}
