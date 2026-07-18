//! `SecretRedactor` scrubs environment-derived secrets from outbound text.
//! Invariant: a secret-bearing variable's value never survives redaction.

#![no_main]

use housebot_bot_response::SecretRedactor;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: (String, String)| {
    let (secret, text) = input;
    let redactor = SecretRedactor::from_vars([("API_KEY".to_string(), secret.clone())]);
    let redacted = redactor.redact(&text);
    // Redaction applies to non-trivial secrets; short values are ignored to
    // avoid mangling ordinary text, so only assert above that threshold.
    if secret.chars().count() >= 8 && text.contains(&secret) {
        assert!(
            !redacted.contains(&secret),
            "secret survived redaction"
        );
    }
});
