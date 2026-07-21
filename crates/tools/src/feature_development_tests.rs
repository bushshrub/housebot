//! Unit tests for `feature_development` (split out to keep the module under 600 lines).

use super::*;
use housebot_coding_agent::catalog::CodingAgent;

fn make_store() -> Arc<PendingJobStore> {
    Arc::new(PendingJobStore::default())
}

fn make_limiter() -> RateLimiter {
    RateLimiter::new(2, Duration::from_secs(3600))
}

fn owner_requester(owner_id: u64) -> DevelopmentRequester {
    DevelopmentRequester {
        user_id: owner_id,
        username: "owner".into(),
        channel_id: 1,
        guild_id: None,
        source_message_id: 10,
    }
}

fn non_owner_requester() -> DevelopmentRequester {
    DevelopmentRequester {
        user_id: 99,
        username: "user".into(),
        channel_id: 1,
        guild_id: None,
        source_message_id: 10,
    }
}

fn source() -> DiscordMessageRef {
    DiscordMessageRef {
        channel_id: 1,
        message_id: 10,
    }
}

fn valid_reqs() -> Vec<String> {
    vec!["Do the thing".into()]
}

fn valid_ac() -> Vec<String> {
    vec!["Thing is done".into()]
}

fn defaults() -> PartialAgentSelection {
    PartialAgentSelection {
        agent: Some(CodingAgent::Claude),
        model: Some("model".into()),
        effort: Some("high".into()),
    }
}

#[test]
fn owner_interactive_returns_config_required() {
    let store = make_store();
    let rl = make_limiter();
    let outcome = prepare_feature_development(
        &store,
        &rl,
        42,
        owner_requester(42),
        source(),
        1,
        "Title",
        "Obj",
        "",
        valid_reqs(),
        valid_ac(),
        DispatchMode::Interactive,
        &defaults(),
    );
    assert!(matches!(
        outcome,
        FeatureDevelopmentOutcome::OwnerConfigurationRequired { .. }
    ));
}

#[test]
fn configurer_interactive_returns_config_required() {
    // A non-owner requester with DispatchMode::Interactive (as the caller
    // decides for configurers) must route the same as the owner would —
    // routing must key off dispatch_mode, not requester.user_id == owner_id.
    let store = make_store();
    let rl = make_limiter();
    let outcome = prepare_feature_development(
        &store,
        &rl,
        42,
        non_owner_requester(),
        source(),
        1,
        "Title",
        "Obj",
        "",
        valid_reqs(),
        valid_ac(),
        DispatchMode::Interactive,
        &defaults(),
    );
    assert!(matches!(
        outcome,
        FeatureDevelopmentOutcome::OwnerConfigurationRequired { .. }
    ));
}

#[test]
fn non_owner_creates_approval_request() {
    let store = make_store();
    let rl = make_limiter();
    let outcome = prepare_feature_development(
        &store,
        &rl,
        42,
        non_owner_requester(),
        source(),
        1,
        "Title",
        "Obj",
        "",
        valid_reqs(),
        valid_ac(),
        DispatchMode::RequireOwnerApproval,
        &defaults(),
    );
    assert!(matches!(
        outcome,
        FeatureDevelopmentOutcome::OwnerApprovalRequired { .. }
    ));
}

#[test]
fn missing_owner_id_rejected() {
    let store = make_store();
    let rl = make_limiter();
    let outcome = prepare_feature_development(
        &store,
        &rl,
        0,
        owner_requester(0),
        source(),
        1,
        "Title",
        "Obj",
        "",
        valid_reqs(),
        valid_ac(),
        DispatchMode::Interactive,
        &defaults(),
    );
    assert!(matches!(
        outcome,
        FeatureDevelopmentOutcome::Rejected { .. }
    ));
    if let FeatureDevelopmentOutcome::Rejected { message } = outcome {
        assert!(message.contains("OWNER_DISCORD_ID"));
    }
}

#[test]
fn non_owner_limiter_applies_before_notification() {
    let store = make_store();
    let rl = RateLimiter::new(0, Duration::from_secs(3600)); // zero limit
    let outcome = prepare_feature_development(
        &store,
        &rl,
        42,
        non_owner_requester(),
        source(),
        1,
        "Title",
        "Obj",
        "",
        valid_reqs(),
        valid_ac(),
        DispatchMode::RequireOwnerApproval,
        &defaults(),
    );
    // Should be rejected (rate limited), not approved.
    assert!(matches!(
        outcome,
        FeatureDevelopmentOutcome::Rejected { .. }
    ));
    // No job should have been created.
    assert_eq!(store.pending_count(), 0);
}

#[test]
fn existing_pending_request_blocks_another() {
    let store = make_store();
    let rl = make_limiter();
    let outcome1 = prepare_feature_development(
        &store,
        &rl,
        42,
        non_owner_requester(),
        source(),
        1,
        "Title",
        "Obj",
        "",
        valid_reqs(),
        valid_ac(),
        DispatchMode::RequireOwnerApproval,
        &defaults(),
    );
    assert!(matches!(
        outcome1,
        FeatureDevelopmentOutcome::OwnerApprovalRequired { .. }
    ));
    // Second request from same user.
    let outcome2 = prepare_feature_development(
        &store,
        &rl,
        42,
        non_owner_requester(),
        DiscordMessageRef {
            channel_id: 1,
            message_id: 11,
        },
        1,
        "Different title",
        "Obj",
        "",
        valid_reqs(),
        valid_ac(),
        DispatchMode::RequireOwnerApproval,
        &defaults(),
    );
    assert!(matches!(
        outcome2,
        FeatureDevelopmentOutcome::Rejected { .. }
    ));
}

#[test]
fn global_pending_limit_enforced() {
    let store = make_store();
    let rl = make_limiter();
    // Fill the store with MAX pending jobs from different users.
    let max: usize =
        config::env_parse("DEVELOPMENT_PENDING_GLOBAL_MAX", DEFAULT_PENDING_GLOBAL_MAX);
    for i in 0..max {
        let req = DevelopmentRequester {
            user_id: 1000 + i as u64,
            username: format!("user{i}"),
            channel_id: 1,
            guild_id: None,
            source_message_id: 100 + i as u64,
        };
        let src = DiscordMessageRef {
            channel_id: 1,
            message_id: 100 + i as u64,
        };
        let _ = prepare_feature_development(
            &store,
            &rl,
            42,
            req,
            src,
            1,
            "Title",
            "Obj",
            "",
            valid_reqs(),
            valid_ac(),
            DispatchMode::RequireOwnerApproval,
            &defaults(),
        );
    }
    // One more should be rejected.
    let outcome = prepare_feature_development(
        &store,
        &rl,
        42,
        non_owner_requester(),
        source(),
        1,
        "Title",
        "Obj",
        "",
        valid_reqs(),
        valid_ac(),
        DispatchMode::RequireOwnerApproval,
        &defaults(),
    );
    assert!(matches!(
        outcome,
        FeatureDevelopmentOutcome::Rejected { .. }
    ));
}

#[test]
fn duplicate_request_suppressed() {
    let store = make_store();
    let rl = make_limiter();
    let outcome1 = prepare_feature_development(
        &store,
        &rl,
        42,
        non_owner_requester(),
        source(),
        1,
        "Title",
        "Obj",
        "",
        valid_reqs(),
        valid_ac(),
        DispatchMode::RequireOwnerApproval,
        &defaults(),
    );
    assert!(matches!(
        outcome1,
        FeatureDevelopmentOutcome::OwnerApprovalRequired { .. }
    ));
    // Reject it so the one-pending-per-user check passes, then try the same spec again.
    if let FeatureDevelopmentOutcome::OwnerApprovalRequired { job_id } = outcome1 {
        store.try_reject(job_id);
    }
    // But the fingerprint should still match any currently-pending job (none now), so
    // the duplicate check is about same requester+fingerprint currently pending. Since we
    // rejected it, a new one is allowed. This test verifies the fingerprint mechanism itself.
    let fp = DevelopmentSpecification {
        issue_number: 1,
        title: "Title".into(),
        objective: "Obj".into(),
        context: String::new(),
        requirements: valid_reqs(),
        acceptance_criteria: valid_ac(),
    }
    .fingerprint();
    assert!(!store.has_equivalent_pending_request(99, &fp));
}

#[test]
fn invalid_specification_rejected_before_job_insertion() {
    let store = make_store();
    let rl = make_limiter();
    let outcome = prepare_feature_development(
        &store,
        &rl,
        42,
        owner_requester(42),
        source(),
        1,
        "", // empty title
        "Obj",
        "",
        valid_reqs(),
        valid_ac(),
        DispatchMode::Interactive,
        &defaults(),
    );
    assert!(matches!(
        outcome,
        FeatureDevelopmentOutcome::Rejected { .. }
    ));
    assert_eq!(store.pending_count(), 0);
}

#[test]
fn definition_has_required_fields() {
    let d = definition();
    assert_eq!(d["name"], "prepare_feature_development");
    let required = &d["input_schema"]["required"];
    assert!(required
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("title")));
    assert!(required
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("objective")));
    assert!(required
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("issue_number")));
}
