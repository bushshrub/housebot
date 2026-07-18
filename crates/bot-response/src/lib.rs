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
        // A single pass is not enough: the replacement's trailing ']' can butt
        // up against the remaining input and re-form a secret that itself
        // starts with ']' (found by the `redact` fuzz target). Scrub until
        // stable, failing closed if an adversarial input never converges.
        const MAX_PASSES: usize = 16;
        let mut output = text.to_string();
        for _ in 0..MAX_PASSES {
            let before = std::mem::take(&mut output);
            output = self.secrets.iter().fold(before.clone(), |acc, secret| {
                acc.replace(secret, "[REDACTED]")
            });
            if output == before {
                return output;
            }
        }
        "[REDACTED]".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redactor(value: &str) -> SecretRedactor {
        SecretRedactor::from_vars([("API_KEY".to_string(), value.to_string())])
    }

    #[test]
    fn redacts_simple_secret() {
        let r = redactor("hunter2hunter2");
        assert_eq!(
            r.redact("the key is hunter2hunter2!"),
            "the key is [REDACTED]!"
        );
    }

    #[test]
    fn short_or_unrelated_vars_are_ignored() {
        let vars = [
            ("API_KEY".to_string(), "short".to_string()),
            ("HOSTNAME".to_string(), "not-a-secret-value".to_string()),
        ];
        let r = SecretRedactor::from_vars(vars);
        assert_eq!(
            r.redact("short not-a-secret-value"),
            "short not-a-secret-value"
        );
    }

    #[test]
    fn replacement_cannot_recombine_into_the_secret() {
        // Regression (found by the `redact` fuzz target): the replacement's
        // trailing ']' could join the remaining input and re-form a secret
        // that starts with ']'.
        let secret = "]XYZAAAA";
        let r = redactor(secret);
        let text = "]XYZAAAAXYZAAAA";
        let redacted = r.redact(text);
        assert!(!redacted.contains(secret), "secret survived: {redacted}");
    }

    #[test]
    fn adversarial_input_fails_closed() {
        let r = redactor("]AAAAAAA");
        let text = "]AAAAAAA".repeat(64) + &"AAAAAAA".repeat(64);
        assert!(!r.redact(&text).contains("]AAAAAAA"));
    }
}
