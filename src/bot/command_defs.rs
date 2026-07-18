//! Slash-command definitions and registration.

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
            "Summarize the conversation into memory and start fresh",
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
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "add",
                "Create or replace a skill",
            )
            .add_sub_option(
                CreateCommandOption::new(
                    CommandOptionType::String,
                    "name",
                    "Skill name (lowercase, numbers, underscores)",
                )
                .required(true),
            )
            .add_sub_option(
                CreateCommandOption::new(
                    CommandOptionType::String,
                    "prompt",
                    "The skill prompt / instructions",
                )
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

pub(crate) async fn register_slash_commands(ctx: &Context) {
    // Register the /config global slash command.
    let config_cmd = CreateCommand::new("config")
        .description("Configure bot settings")
        // ── channel subcommand group ─────────────────────────────────────
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::SubCommandGroup,
                "channel",
                "Manage which channels the bot responds in (server-wide)",
            )
            .add_sub_option(CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "list",
                "Show the current channel allowlist",
            ))
            .add_sub_option(
                CreateCommandOption::new(
                    CommandOptionType::SubCommand,
                    "add",
                    "Add a channel to the allowlist",
                )
                .add_sub_option(
                    CreateCommandOption::new(
                        CommandOptionType::Channel,
                        "channel",
                        "The channel to allow",
                    )
                    .required(true),
                ),
            )
            .add_sub_option(
                CreateCommandOption::new(
                    CommandOptionType::SubCommand,
                    "remove",
                    "Remove a channel from the allowlist",
                )
                .add_sub_option(
                    CreateCommandOption::new(
                        CommandOptionType::Channel,
                        "channel",
                        "The channel to remove",
                    )
                    .required(true),
                ),
            )
            .add_sub_option(CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "clear",
                "Remove all channel restrictions (bot responds everywhere)",
            )),
        )
        // ── leaderboard subcommand group ────────────────────────────────
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::SubCommandGroup,
                "leaderboard",
                "Configure token leaderboard access (administrators only)",
            )
            .add_sub_option(
                CreateCommandOption::new(
                    CommandOptionType::SubCommand,
                    "visibility",
                    "Set whether leaderboard responses are public, private, or restricted",
                )
                .add_sub_option(
                    CreateCommandOption::new(
                        CommandOptionType::String,
                        "mode",
                        "Leaderboard visibility mode",
                    )
                    .required(true)
                    .add_string_choice("Public channel response", "public")
                    .add_string_choice("Private response", "private")
                    .add_string_choice("Restricted to roles", "restricted"),
                ),
            )
            .add_sub_option(
                CreateCommandOption::new(
                    CommandOptionType::SubCommand,
                    "role_add",
                    "Allow a role to use the leaderboard in restricted mode",
                )
                .add_sub_option(
                    CreateCommandOption::new(CommandOptionType::Role, "role", "Role to allow")
                        .required(true),
                ),
            )
            .add_sub_option(
                CreateCommandOption::new(
                    CommandOptionType::SubCommand,
                    "role_remove",
                    "Remove a role from restricted leaderboard access",
                )
                .add_sub_option(
                    CreateCommandOption::new(CommandOptionType::Role, "role", "Role to remove")
                        .required(true),
                ),
            )
            .add_sub_option(CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "role_list",
                "List roles allowed to use the leaderboard",
            )),
        )
        // ── personality subcommand ───────────────────────────────────────
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "personality",
                "Set or clear your personal bot personality / tone override",
            )
            .add_sub_option(CreateCommandOption::new(
                CommandOptionType::String,
                "text",
                "Personality description (omit to clear your override)",
            )),
        )
        // ── followup subcommand ──────────────────────────────────────────
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "followup",
                "Control whether the bot replies without a ping during active conversations",
            )
            .add_sub_option(
                CreateCommandOption::new(
                    CommandOptionType::Boolean,
                    "enabled",
                    "Enable or disable follow-up replies",
                )
                .required(true),
            )
            .add_sub_option(CreateCommandOption::new(
                CommandOptionType::Integer,
                "timeout",
                "Seconds to keep the conversation open without a ping (default 300)",
            )),
        );

    if let Err(e) = Command::create_global_command(&ctx.http, config_cmd).await {
        tracing::error!("Failed to register /config slash command: {e}");
    }
    let labs_cmd = CreateCommand::new("labs")
        .description("Enable experimental bot features")
        .add_option(CreateCommandOption::new(
            CommandOptionType::SubCommand,
            "list",
            "List experimental features and their status",
        ))
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "pagination",
                "Toggle paginated LLM responses",
            )
            .add_sub_option(
                CreateCommandOption::new(
                    CommandOptionType::Boolean,
                    "enabled",
                    "Enable or disable paginated responses",
                )
                .required(true),
            ),
        );
    if let Err(e) = Command::create_global_command(&ctx.http, labs_cmd).await {
        tracing::error!(target: "housebot::labs::registration", "Failed to register /labs slash command: {e}");
    }
    let mut effort_level_option = CreateCommandOption::new(
        CommandOptionType::String,
        "level",
        "Thinking effort level (omit to show the current setting)",
    );
    for mode in ThinkingMode::ALL {
        effort_level_option = effort_level_option
            .add_string_choice(format!("{mode} ({})", mode.budget_label()), mode.as_str());
    }
    let effort_cmd = CreateCommand::new("effort")
        .description("Set how much thinking the model does before replying")
        .add_option(effort_level_option);
    if let Err(e) = Command::create_global_command(&ctx.http, effort_cmd).await {
        tracing::error!("Failed to register /effort slash command: {e}");
    }
    let tool_ban_cmd = CreateCommand::new("tool_ban")
        .description("Propose and vote on user-specific tool restrictions")
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "propose",
                "Propose restricting a user from one tool",
            )
            .add_sub_option(
                CreateCommandOption::new(CommandOptionType::User, "user", "User to restrict")
                    .required(true),
            )
            .add_sub_option(
                CreateCommandOption::new(
                    CommandOptionType::String,
                    "tool",
                    "Tool name — start typing for suggestions",
                )
                .required(true)
                .set_autocomplete(true),
            ),
        )
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "vote",
                "Vote on an open tool-ban proposal",
            )
            .add_sub_option(
                CreateCommandOption::new(
                    CommandOptionType::String,
                    "proposal",
                    "Proposal ID shown by propose or status",
                )
                .required(true),
            )
            .add_sub_option(
                CreateCommandOption::new(
                    CommandOptionType::Boolean,
                    "approve",
                    "True to approve the ban; false to reject it",
                )
                .required(true),
            ),
        )
        .add_option(CreateCommandOption::new(
            CommandOptionType::SubCommand,
            "status",
            "Show active bans and open proposals",
        ));
    if let Err(error) = Command::create_global_command(&ctx.http, tool_ban_cmd).await {
        tracing::error!(%error, "Failed to register /tool_ban slash command");
    }
    let lua_cmd = CreateCommand::new("lua")
            .description(
                "Run a sandboxed Lua script; use graph.node/edge to render a diagram (requires the Scripting role)",
            )
            .add_option(
                CreateCommandOption::new(
                    CommandOptionType::String,
                    "script",
                    "Lua code to run (a ```lua code block``` is accepted)",
                )
                .required(true),
            );
    if let Err(e) = Command::create_global_command(&ctx.http, lua_cmd.clone()).await {
        tracing::error!("Failed to register /lua slash command: {e}");
    }
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
    if let Some(guild_id) = guild_id {
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
    for command in [
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
            )
            .add_option(
                CreateCommandOption::new(
                    CommandOptionType::SubCommand,
                    "proactive",
                    "Toggle proactive assistance",
                )
                .add_sub_option(
                    CreateCommandOption::new(
                        CommandOptionType::Boolean,
                        "enabled",
                        "Enable or disable proactive assistance",
                    )
                    .required(true),
                ),
            ),
        storage_command_definition(),
    ] {
        if let Err(e) = Command::create_global_command(&ctx.http, command).await {
            tracing::error!("Failed to register slash command: {e}");
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
