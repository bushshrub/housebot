//! Tool ban/restore command definitions.

//! Slash-command definitions and registration.

use super::*;

pub(crate) fn tool_ban_command_definition() -> CreateCommand {
    CreateCommand::new("tool_ban")
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
        ))
}

pub(crate) fn tool_restore_command_definition() -> CreateCommand {
    CreateCommand::new("tool_restore")
        .description("Propose and vote on restoring tool access for a restricted user")
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "propose",
                "Propose restoring a user's access to one tool",
            )
            .add_sub_option(
                CreateCommandOption::new(CommandOptionType::User, "user", "User to restore")
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
                "Vote on an open tool-restore proposal",
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
                    "True to approve the restoration; false to reject it",
                )
                .required(true),
            ),
        )
        .add_option(CreateCommandOption::new(
            CommandOptionType::SubCommand,
            "status",
            "Show active bans and open restore proposals",
        ))
}
