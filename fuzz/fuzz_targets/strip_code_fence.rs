//! `strip_code_fence` trims user-supplied /lua scripts. Invariants: no
//! panic, and the result is always a substring of the input.

#![no_main]

use housebot_lua_engine::strip_code_fence;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|script: &str| {
    let stripped = strip_code_fence(script);
    assert!(script.contains(stripped), "result must be a substring");
});
