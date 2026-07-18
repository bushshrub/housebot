//! Tool names and arguments arrive from the model and MCP servers, i.e.
//! untrusted input. `tool_status` previously panicked on multi-byte
//! characters at the truncation boundary — this target pins that class of
//! bug down permanently.

#![no_main]

use housebot_bot_formatting::{lang_ext, tool_hint, tool_status};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: (&str, &str)| {
    let (name, raw) = input;
    let _ = tool_status(name);
    let _ = lang_ext(name);
    let args = serde_json::from_str(raw).unwrap_or(serde_json::json!({ "query": raw }));
    let _ = tool_hint(name, &args);
});
