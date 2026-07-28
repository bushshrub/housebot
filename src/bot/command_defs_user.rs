//! Per-user command definitions: personalize, labs, lua.

//! Slash-command definitions and registration.

use super::*;

pub(crate) fn personalize_command_definition() -> CreateCommand {
    CreateCommand::new("personalize")
        .description("Personal bot settings any user can change")
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
        )
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "proactive",
                "Control whether the bot may respond to your messages unprompted",
            )
            .add_sub_option(
                CreateCommandOption::new(
                    CommandOptionType::Boolean,
                    "enabled",
                    "Enable or disable proactive assistance",
                )
                .required(true),
            ),
        )
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "progress",
                "Control whether intermediate progress updates are shown",
            )
            .add_sub_option(
                CreateCommandOption::new(
                    CommandOptionType::Boolean,
                    "enabled",
                    "Show reasoning, queue, generating, and tool progress",
                )
                .required(true),
            )
            .add_sub_option(CreateCommandOption::new(
                CommandOptionType::User,
                "user",
                "User to configure (server administrators and bot configurers only)",
            )),
        )
}

pub(crate) fn labs_command_definition() -> CreateCommand {
    CreateCommand::new("labs")
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
        )
}

pub(crate) fn lua_command_definition() -> CreateCommand {
    CreateCommand::new("lua")
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
        )
}
