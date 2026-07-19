//! Integration tests for the public `housebot-llm` surface.

use housebot_llm::ThinkingMode;

#[test]
fn every_mode_round_trips_through_its_string_form() {
    for mode in ThinkingMode::ALL {
        assert_eq!(mode.as_str().parse::<ThinkingMode>(), Ok(mode));
        assert_eq!(mode.to_string(), mode.as_str());
    }
}

#[test]
fn parsing_is_case_insensitive_and_rejects_unknown() {
    assert_eq!("HIGH".parse::<ThinkingMode>(), Ok(ThinkingMode::High));
    assert!("blazing".parse::<ThinkingMode>().is_err());
}

#[test]
fn completion_budget_always_reserves_room_for_the_answer() {
    for mode in ThinkingMode::ALL {
        let ceiling = mode.max_completion_tokens();
        match mode.budget_tokens() {
            Some(budget) => assert!(ceiling > budget, "{mode} leaves no room for a reply"),
            None => assert!(ceiling >= 32_768),
        }
    }
}

#[test]
fn reasoning_field_is_capped_only_for_bounded_modes() {
    assert_eq!(
        ThinkingMode::Instant.reasoning_field(),
        serde_json::json!({"enabled": false})
    );
    assert_eq!(
        ThinkingMode::Low.reasoning_field(),
        serde_json::json!({"enabled": true, "max_tokens": 2048})
    );
    assert_eq!(
        ThinkingMode::Max.reasoning_field(),
        serde_json::json!({"enabled": true})
    );
}
