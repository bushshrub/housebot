//! HMAC authentication for the feature-development completion webhook.
//!
//! The dispatch workflows (`claude-dispatch.yml`, `opencode-dispatch.yml`) and the
//! bot process share a signing key (`DEV_NOTIFY_SIGNING_KEY`). The workflow signs
//! the completion footer; the bot verifies the signature before trusting the
//! embedded `requester_id` and DMing anyone — a channel match alone doesn't prove
//! the message came from the dispatch workflow rather than some other webhook
//! posting into the same channel.

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Compute the hex-encoded HMAC-SHA256 signature over the canonical message.
pub fn sign(key: &[u8], requester_id: u64, issue: u64, status: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC-SHA256 accepts a key of any length");
    mac.update(canonical_message(requester_id, issue, status).as_bytes());
    hex_encode(&mac.finalize().into_bytes())
}

/// Verify `sig_hex` against the canonical message, in constant time.
pub fn verify(key: &[u8], requester_id: u64, issue: u64, status: &str, sig_hex: &str) -> bool {
    let Some(expected) = hex_decode(sig_hex) else {
        return false;
    };
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC-SHA256 accepts a key of any length");
    mac.update(canonical_message(requester_id, issue, status).as_bytes());
    mac.verify_slice(&expected).is_ok()
}

fn canonical_message(requester_id: u64, issue: u64, status: &str) -> String {
    format!("requester_id={requester_id} issue={issue} status={status}")
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.is_ascii() || s.len() % 2 != 0 {
        return None;
    }
    s.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_then_verify_round_trips() {
        let sig = sign(b"secret", 123, 42, "success");
        assert!(verify(b"secret", 123, 42, "success", &sig));
    }

    #[test]
    fn verify_rejects_wrong_key() {
        let sig = sign(b"secret", 123, 42, "success");
        assert!(!verify(b"other-secret", 123, 42, "success", &sig));
    }

    #[test]
    fn verify_rejects_tampered_fields() {
        let sig = sign(b"secret", 123, 42, "success");
        assert!(!verify(b"secret", 999, 42, "success", &sig));
        assert!(!verify(b"secret", 123, 42, "failure", &sig));
    }

    #[test]
    fn verify_rejects_malformed_signature() {
        assert!(!verify(b"secret", 123, 42, "success", "not-hex"));
        assert!(!verify(b"secret", 123, 42, "success", "abc"));
        assert!(!verify(b"secret", 123, 42, "success", ""));
    }

    #[test]
    fn verify_rejects_non_ascii_signature_without_panicking() {
        // "aéx" is 4 bytes (even length) but "é" isn't a char boundary at
        // byte 2, so naive &s[i..i+2] slicing would panic here.
        assert!(!verify(b"secret", 123, 42, "success", "aéx"));
    }
}
