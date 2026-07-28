//! The environment passed through to a redeployed housebot container.

use crate::*;

pub(crate) const HOUSEBOT_ENV_VARS: &[&str] = &[
    "DISCORD_BOT_TOKEN",
    "OWNER_DISCORD_ID",
    "DEPLOYMENT_GUILD_ID",
    "DATABASE_URL",
    "DATABASE_CONNECT_MAX_ATTEMPTS",
    "DATABASE_CONNECT_RETRY_SECS",
    "DATABASE_CONNECT_TIMEOUT_SECS",
    "LLM_BASE_URL",
    "LLM_MODEL",
    "LLM_API_KEY",
    "MAX_HISTORY_TURNS",
    "MAX_CONTEXT_TOKENS",
    "CONVERSATION_IDLE_TIMEOUT",
    "JELLYFIN_URL",
    "JELLYFIN_API_KEY",
    "LLAMA_CPP_URL",
    "LLAMA_CPP_MODEL",
    "GITHUB_APP_ID",
    "GITHUB_APP_PRIVATE_KEY",
    "GITHUB_INSTALLATION_ID",
    "GITHUB_REPO",
];

pub(crate) fn housebot_env() -> Vec<(String, String)> {
    let mut values: HashMap<String, String> = HOUSEBOT_ENV_VARS
        .iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| ((*name).into(), value))
        })
        .collect();

    // Read the mounted deployment configuration at deploy time. This lets an
    // edited .env take effect without restarting the deployment bot itself.
    for path in ["/app/.env", ".env"] {
        if let Ok(contents) = std::fs::read_to_string(path) {
            for (name, value) in parse_dotenv(&contents) {
                if HOUSEBOT_ENV_VARS.contains(&name.as_str()) {
                    values.insert(name, value);
                }
            }
        }
    }

    HOUSEBOT_ENV_VARS
        .iter()
        .filter_map(|name| values.remove(*name).map(|value| ((*name).into(), value)))
        .collect()
}

pub(crate) fn parse_dotenv(contents: &str) -> Vec<(String, String)> {
    contents
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let line = line.strip_prefix("export ").unwrap_or(line);
            let (name, value) = line.split_once('=')?;
            let name = name.trim();
            if name.is_empty() || name.starts_with('#') {
                return None;
            }
            let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
            Some((name.to_string(), value.to_string()))
        })
        .collect()
}
