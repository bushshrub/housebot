//! `extract_code_files` regex-scans arbitrary model output for code fences.
//! Invariants: no panic, and extracted bytes came from the input.

#![no_main]

use housebot_bot_formatting::extract_code_files;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    let (modified, files) = extract_code_files(text);
    for (name, bytes) in &files {
        assert!(!name.is_empty());
        assert!(bytes.len() <= text.len());
        assert!(
            bytes.is_empty()
                || text
                    .as_bytes()
                    .windows(bytes.len())
                    .any(|window| window == bytes),
            "extracted bytes must come from input"
        );
    }
    let _ = modified;
});
