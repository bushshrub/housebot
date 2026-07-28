//! Bot- and server-configuration command definitions.

//! Slash-command definitions and registration.

use super::*;

pub(crate) fn config_command_definition() -> CreateCommand {
    CreateCommand::new("config")
    .description("Configure the bot (authorized configurers only)")
    // ── proactive subcommand (global proactive kill-switch) ──────────
    .add_option(
        CreateCommandOption::new(
            CommandOptionType::SubCommand,
            "proactive",
            "Enable or disable proactive assistance for all users (configurers only)",
        )
        .add_sub_option(
            CreateCommandOption::new(
                CommandOptionType::Boolean,
                "enabled",
                "Whether proactive assistance is available to anyone",
            )
            .required(true),
        ),
    )
    // ── dev_notify_channel subcommand (feature-development completion) ─
    .add_option(
        CreateCommandOption::new(
            CommandOptionType::SubCommand,
            "dev_notify_channel",
            "Set which channel to watch for feature-development completion notices (omit to disable)",
        )
        .add_sub_option(CreateCommandOption::new(
            CommandOptionType::Channel,
            "channel",
            "Channel receiving the dispatch workflows' completion webhook",
        )),
    )
    // ── access subcommand group ──────────────────────────────────────
    .add_option(
        CreateCommandOption::new(
            CommandOptionType::SubCommandGroup,
            "access",
            "Manage which users are allowed to configure the bot (owner is always allowed)",
        )
        .add_sub_option(
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "allow",
                "Allow a user to configure the bot",
            )
            .add_sub_option(
                CreateCommandOption::new(CommandOptionType::User, "user", "User to allow")
                    .required(true),
            ),
        )
        .add_sub_option(
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "revoke",
                "Revoke a user's permission to configure the bot",
            )
            .add_sub_option(
                CreateCommandOption::new(CommandOptionType::User, "user", "User to revoke")
                    .required(true),
            ),
        )
        .add_sub_option(CreateCommandOption::new(
            CommandOptionType::SubCommand,
            "list",
            "List the users allowed to configure the bot",
        )),
    )
    // ── user policy subcommand group ─────────────────────────────────
    .add_option(
        CreateCommandOption::new(
            CommandOptionType::SubCommandGroup,
            "user",
            "Per-user bot policies (configurers only)",
        )
        .add_sub_option(
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "limit",
                "Cap a user's maximum output tokens (omit max_tokens to remove the cap)",
            )
            .add_sub_option(
                CreateCommandOption::new(CommandOptionType::User, "user", "User to limit")
                    .required(true),
            )
            .add_sub_option(CreateCommandOption::new(
                CommandOptionType::Integer,
                "max_tokens",
                "Maximum output tokens per response (omit to remove the cap)",
            )),
        )
        .add_sub_option(
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "respond",
                "Control whether the bot responds to a user at all",
            )
            .add_sub_option(
                CreateCommandOption::new(CommandOptionType::User, "user", "Target user")
                    .required(true),
            )
            .add_sub_option(
                CreateCommandOption::new(
                    CommandOptionType::Boolean,
                    "enabled",
                    "Whether the bot responds to this user",
                )
                .required(true),
            ),
        )
        .add_sub_option(
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "show",
                "Show a user's current bot policy",
            )
            .add_sub_option(
                CreateCommandOption::new(CommandOptionType::User, "user", "Target user")
                    .required(true),
            ),
        ),
    )
}

pub(crate) fn server_config_command_definition() -> CreateCommand {
    CreateCommand::new("server-config")
        .description("Configure server-scoped bot settings (server administrators and configurers)")
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
                "Configure token leaderboard access",
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
        // ── bot_pings subcommand ─────────────────────────────────────────
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "bot_pings",
                "Control whether the bot responds to @-mentions from other bots",
            )
            .add_sub_option(
                CreateCommandOption::new(
                    CommandOptionType::Boolean,
                    "enabled",
                    "Enable or disable responses to other bots",
                )
                .required(true),
            ),
        )
        // ── proactive subcommand ─────────────────────────────────────────
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "proactive",
                "Control whether proactive assistance is allowed in this server",
            )
            .add_sub_option(
                CreateCommandOption::new(
                    CommandOptionType::Boolean,
                    "enabled",
                    "Whether users may enable proactive assistance here",
                )
                .required(true),
            ),
        )
}
