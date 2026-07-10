//! Outbound-response security helpers independent of Discord.

/// Redacts known secret values (drawn from the environment) from outbound text.
pub struct SecretRedactor {
    secrets: Vec<String>,
}

impl SecretRedactor {
    const KEYWORDS: &'static [&'static str] = &[
        "token", "key", "secret", "password", "dsn", "api_key", "oauth",
    ];

    /// Build from the process environment.
    pub fn from_env() -> Self {
        Self::from_vars(std::env::vars())
    }

    /// Build from an explicit iterator of `(name, value)` pairs.
    pub fn from_vars(vars: impl IntoIterator<Item = (String, String)>) -> Self {
        let secrets = vars
            .into_iter()
            .filter(|(name, value)| {
                value.len() >= 8
                    && Self::KEYWORDS
                        .iter()
                        .any(|keyword| name.to_lowercase().contains(keyword))
            })
            .map(|(_, value)| value)
            .collect();
        Self { secrets }
    }

    /// Replace every known secret value with `[REDACTED]`.
    pub fn redact(&self, text: &str) -> String {
        self.secrets
            .iter()
            .fold(text.to_string(), |output, secret| {
                output.replace(secret, "[REDACTED]")
            })
    }
}
