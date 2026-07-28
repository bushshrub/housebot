//! Unit tests for `lib` (split out to keep the module under 400 lines).

use super::*;

fn chunk(json: &str) -> StreamChunk {
    serde_json::from_str(json).unwrap()
}

#[test]
fn accumulator_collects_text() {
    let mut acc = Accumulator::default();
    acc.apply(chunk(r#"{"choices":[{"delta":{"content":"Hel"}}]}"#));
    acc.apply(chunk(r#"{"choices":[{"delta":{"content":"lo"}}]}"#));
    acc.apply(chunk(
        r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
    ));
    let done = acc.finish();
    assert_eq!(done.content.as_deref(), Some("Hello"));
    assert_eq!(done.finish_reason.as_deref(), Some("stop"));
    assert!(done.tool_calls.is_empty());
}

#[test]
fn accumulator_assembles_tool_calls() {
    let mut acc = Accumulator::default();
    acc.apply(chunk(
        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"trans","arguments":"{\"a\":"}}]}}]}"#,
    ));
    acc.apply(chunk(
        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"1}"}}]},"finish_reason":"tool_calls"}]}"#,
    ));
    let done = acc.finish();
    assert_eq!(done.finish_reason.as_deref(), Some("tool_calls"));
    assert_eq!(done.tool_calls.len(), 1);
    assert_eq!(done.tool_calls[0].id, "call_1");
    assert_eq!(done.tool_calls[0].name, "trans");
    assert_eq!(done.tool_calls[0].arguments, "{\"a\":1}");
}

#[test]
fn apply_returns_content_delta() {
    let mut acc = Accumulator::default();
    let d = acc.apply(chunk(r#"{"choices":[{"delta":{"content":"hi"}}]}"#));
    assert_eq!(d.as_deref(), Some("hi"));
}

#[test]
fn parse_sse_line_handles_done_and_data() {
    assert!(parse_sse_line("data: [DONE]").is_none());
    assert!(parse_sse_line(": comment").is_none());
    assert!(parse_sse_line(r#"data: {"choices":[]}"#).is_some());
}

#[test]
fn thinking_mode_budgets_match_spec() {
    assert_eq!(ThinkingMode::Instant.budget_tokens(), Some(0));
    assert_eq!(ThinkingMode::Low.budget_tokens(), Some(2_048));
    assert_eq!(ThinkingMode::Medium.budget_tokens(), Some(4_096));
    assert_eq!(ThinkingMode::High.budget_tokens(), Some(8_192));
    assert_eq!(ThinkingMode::XHigh.budget_tokens(), Some(16_384));
    assert_eq!(ThinkingMode::Max.budget_tokens(), None);
}

#[test]
fn thinking_mode_parses_and_displays() {
    for mode in ThinkingMode::ALL {
        assert_eq!(mode.as_str().parse::<ThinkingMode>(), Ok(mode));
    }
    assert_eq!("XHIGH".parse::<ThinkingMode>(), Ok(ThinkingMode::XHigh));
    assert!("turbo".parse::<ThinkingMode>().is_err());
}

#[test]
fn thinking_mode_serde_roundtrip() {
    assert_eq!(
        serde_json::to_string(&ThinkingMode::XHigh).unwrap(),
        "\"xhigh\""
    );
    assert_eq!(
        serde_json::from_str::<ThinkingMode>("\"max\"").unwrap(),
        ThinkingMode::Max
    );
}

#[test]
fn thinking_mode_max_tokens_leave_room_for_answer() {
    assert_eq!(ThinkingMode::Instant.max_completion_tokens(), 4_096);
    assert_eq!(ThinkingMode::Low.max_completion_tokens(), 2_048 + 4_096);
    assert_eq!(ThinkingMode::Max.max_completion_tokens(), 32_768);
}

#[test]
fn reasoning_field_caps_bounded_modes_only() {
    assert_eq!(
        ThinkingMode::Instant.reasoning_field(),
        serde_json::json!({"enabled": false})
    );
    assert_eq!(
        ThinkingMode::Medium.reasoning_field(),
        serde_json::json!({"enabled": true, "max_tokens": 4096})
    );
    assert_eq!(
        ThinkingMode::Max.reasoning_field(),
        serde_json::json!({"enabled": true})
    );
}

#[test]
fn empty_tool_calls_are_dropped() {
    // A slot that never received a name must not surface as a tool call.
    let mut acc = Accumulator::default();
    acc.apply(chunk(
        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"x"}]}}]}"#,
    ));
    assert!(acc.finish().tool_calls.is_empty());
}
