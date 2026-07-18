//! `split_text` chunks arbitrary model output for Discord's message length
//! cap. Invariants: terminates, never yields an oversized or empty chunk.

#![no_main]

use housebot_bot_formatting::split_text;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: (&str, u16)| {
    let (text, limit) = input;
    let limit = limit as usize;
    let chunks = split_text(text, limit);
    let effective = limit.max(1);
    for chunk in &chunks {
        let len = chunk.chars().count();
        assert!(len <= effective, "chunk of {len} chars exceeds limit {effective}");
    }
    if !text.is_empty() {
        assert!(!chunks.is_empty(), "non-empty input produced no chunks");
        assert!(
            chunks.iter().all(|chunk| !chunk.is_empty()),
            "non-empty input produced an empty chunk"
        );
    }
});
