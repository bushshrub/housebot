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
fn only_the_supported_effort_levels_exist() {
    let levels: Vec<&str> = ThinkingMode::ALL.iter().map(|m| m.as_str()).collect();
    assert_eq!(levels, ["low", "medium", "xhigh"]);
    assert!("blazing".parse::<ThinkingMode>().is_err());
}
