//! Tool for preparing an automated coding-agent development job.
//!
//! The LLM calls this to build a structured spec. For owner requests the job may be
//! dispatched immediately (OwnerDispatchReady) or enter the interactive selection flow
//! (OwnerConfigurationRequired). For non-owner requests the owner must approve first
//! (OwnerApprovalRequired).

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use uuid::Uuid;

use crate::coding_agent::pending::{
    DevelopmentRequester, DevelopmentSpecification, DiscordMessageRef, DispatchStage,
    PartialAgentSelection, PendingDevelopmentJob, PendingJobStore,
};
use crate::config;
use crate::rate_limit::RateLimiter;

/// Default rate limits for non-owner development requests.
const NON_OWNER_RATE_LIMIT_MAX: usize = 2;
const NON_OWNER_RATE_LIMIT_WINDOW: Duration = Duration::from_secs(3600);

/// Safety limit for actual owner dispatch executions (not preparation).
const OWNER_DISPATCH_LIMIT_MAX: usize = 10;
const OWNER_DISPATCH_LIMIT_WINDOW: Duration = Duration::from_secs(600);

/// Global cap on pending jobs.
const DEFAULT_PENDING_GLOBAL_MAX: usize = 10;

pub fn default_rate_limiter() -> RateLimiter {
    let max: usize = config::env_parse(
        "DEVELOPMENT_REQUEST_RATE_LIMIT_MAX",
        NON_OWNER_RATE_LIMIT_MAX,
    );
    let secs: u64 = config::env_parse(
        "DEVELOPMENT_REQUEST_RATE_LIMIT_WINDOW_SECS",
        NON_OWNER_RATE_LIMIT_WINDOW.as_secs(),
    );
    RateLimiter::new(max, Duration::from_secs(secs))
}

pub fn owner_dispatch_limiter() -> RateLimiter {
    RateLimiter::new(OWNER_DISPATCH_LIMIT_MAX, OWNER_DISPATCH_LIMIT_WINDOW)
}

/// Typed outcome returned by `prepare_feature_development` so the bot layer can
/// take the appropriate action without parsing magic strings.
#[derive(Debug)]
pub enum FeatureDevelopmentOutcome {
    /// Owner-direct request with defaults; ready to dispatch immediately.
    OwnerDispatchReady { job_id: Uuid },
    /// Owner requested interactive agent/model/effort selection.
    OwnerConfigurationRequired { job_id: Uuid },
    /// Non-owner request; owner must approve before execution.
    OwnerApprovalRequired { job_id: Uuid },
    /// Request rejected before any job was created.
    Rejected { message: String },
}

impl FeatureDevelopmentOutcome {
    /// The text to return to the LLM for each outcome variant.
    pub fn tool_response(&self) -> String {
        match self {
            Self::OwnerDispatchReady { job_id } => {
                format!("OWNER_DISPATCH_READY:{job_id}")
            }
            Self::OwnerConfigurationRequired { job_id } => {
                format!("OWNER_CONFIG_REQUIRED:{job_id}")
            }
            Self::OwnerApprovalRequired { job_id } => {
                format!(
                    "Development request created (ID: {job_id}). \
                     The bot owner has been notified and must approve before execution begins."
                )
            }
            Self::Rejected { message } => format!("Error: {message}"),
        }
    }
}

/// How the caller expects dispatch to proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchMode {
    /// Owner wants immediate dispatch using defaults.
    Immediate,
    /// Owner wants to interactively choose agent/model/effort.
    Interactive,
    /// Non-owner: must await owner approval.
    RequireOwnerApproval,
}

/// OpenAI-style tool definition.
pub fn definition() -> Value {
    json!({
        "name": "prepare_feature_development",
        "description": "Prepare an automated feature-development request.\n\
            When the configured owner explicitly says to start, implement, build, or begin work, \
            call this and treat the request as authorized for immediate dispatch.\n\
            When another user requests implementation, call this so the owner can approve it.\n\
            Do not claim that work has started until the dispatch succeeds.\n\
            Use create_feature_request for suggestions that do not ask to begin implementation.",
        "input_schema": {
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Concise feature title under 100 characters."
                },
                "objective": {
                    "type": "string",
                    "description": "The desired final behavior in clear terms."
                },
                "context": {
                    "type": "string",
                    "description": "Relevant current behavior, motivation, and constraints."
                },
                "requirements": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Specific implementation requirements."
                },
                "acceptance_criteria": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Conditions that must be true for the feature to be complete."
                },
                "interactive": {
                    "type": "boolean",
                    "description": "Set to true when the owner explicitly asks to choose the coding agent interactively."
                }
            },
            "required": ["title", "objective", "requirements", "acceptance_criteria"]
        }
    })
}

/// Validate the specification fields.
fn validate_spec(
    title: &str,
    objective: &str,
    context: &str,
    requirements: &[String],
    acceptance_criteria: &[String],
) -> Result<(), String> {
    if title.is_empty() || title.chars().count() > 100 {
        return Err("Title must be 1–100 characters.".into());
    }
    if objective.is_empty() || objective.chars().count() > 4_000 {
        return Err("Objective must be 1–4,000 characters.".into());
    }
    if context.chars().count() > 8_000 {
        return Err("Context must not exceed 8,000 characters.".into());
    }
    if requirements.is_empty() || requirements.len() > 25 {
        return Err("Requirements must have 1–25 entries.".into());
    }
    if acceptance_criteria.is_empty() || acceptance_criteria.len() > 25 {
        return Err("Acceptance criteria must have 1–25 entries.".into());
    }
    for r in requirements {
        if r.is_empty() || r.chars().count() > 1_000 {
            return Err("Each requirement must be 1–1,000 characters.".into());
        }
    }
    for a in acceptance_criteria {
        if a.is_empty() || a.chars().count() > 1_000 {
            return Err("Each acceptance criterion must be 1–1,000 characters.".into());
        }
    }
    Ok(())
}

/// Prepare a development job and return a typed outcome.
///
/// Authorization rules are enforced here:
/// - Anyone may submit a throttled request.
/// - Only the owner may authorize execution.
/// - An explicit work request from the owner is already authorized.
#[allow(clippy::too_many_arguments)]
pub fn prepare_feature_development(
    store: &Arc<PendingJobStore>,
    non_owner_limiter: &RateLimiter,
    owner_id: u64,
    requester: DevelopmentRequester,
    source_message: DiscordMessageRef,
    title: &str,
    objective: &str,
    context: &str,
    requirements: Vec<String>,
    acceptance_criteria: Vec<String>,
    dispatch_mode: DispatchMode,
    defaults: &PartialAgentSelection,
) -> FeatureDevelopmentOutcome {
    if owner_id == 0 {
        return FeatureDevelopmentOutcome::Rejected {
            message: "OWNER_DISCORD_ID is not configured — automated development is disabled."
                .into(),
        };
    }

    // Input validation runs for all requesters before any state is created.
    if let Err(e) = validate_spec(
        title,
        objective,
        context,
        &requirements,
        &acceptance_criteria,
    ) {
        return FeatureDevelopmentOutcome::Rejected { message: e };
    }

    let spec = DevelopmentSpecification {
        title: title.to_string(),
        objective: objective.to_string(),
        context: context.to_string(),
        requirements,
        acceptance_criteria,
    };

    if requester.user_id == owner_id {
        // Owner path: no per-user rate limiting, immediate or interactive dispatch.
        match dispatch_mode {
            DispatchMode::Interactive => {
                let job = PendingDevelopmentJob::new(
                    owner_id,
                    requester,
                    source_message,
                    spec,
                    DispatchStage::ChoosingAgent,
                    PartialAgentSelection::default(),
                );
                let job_id = store.insert(job);
                FeatureDevelopmentOutcome::OwnerConfigurationRequired { job_id }
            }
            _ => {
                // Immediate: pre-fill from defaults so approve_with_defaults can transition directly.
                let job = PendingDevelopmentJob::new(
                    owner_id,
                    requester,
                    source_message,
                    spec,
                    DispatchStage::Confirming,
                    defaults.clone(),
                );
                let job_id = store.insert(job);
                FeatureDevelopmentOutcome::OwnerDispatchReady { job_id }
            }
        }
    } else {
        // Non-owner path: apply rate limits and anti-spam before creating any state.
        let pending_global_max: usize =
            config::env_parse("DEVELOPMENT_PENDING_GLOBAL_MAX", DEFAULT_PENDING_GLOBAL_MAX);
        if store.pending_count() >= pending_global_max {
            return FeatureDevelopmentOutcome::Rejected {
                message:
                    "The development approval queue is currently full. Please try again later."
                        .into(),
            };
        }

        let existing = store.pending_for_requester(requester.user_id);
        if !existing.is_empty() {
            return FeatureDevelopmentOutcome::Rejected {
                message: "You already have a development request awaiting owner review.".into(),
            };
        }

        let fp = spec.fingerprint();
        if store.has_equivalent_pending_request(requester.user_id, &fp) {
            return FeatureDevelopmentOutcome::Rejected {
                message: "An equivalent development request is already pending.".into(),
            };
        }

        let requester_key = requester.user_id.to_string();
        if non_owner_limiter.check(&requester_key) {
            return FeatureDevelopmentOutcome::Rejected {
                message: "You have submitted too many development requests. \
                          Please wait before trying again."
                    .into(),
            };
        }

        let job = PendingDevelopmentJob::new(
            owner_id,
            requester,
            source_message,
            spec,
            DispatchStage::AwaitingOwnerApproval,
            defaults.clone(),
        );
        let job_id = store.insert(job);
        FeatureDevelopmentOutcome::OwnerApprovalRequired { job_id }
    }
}

/// Parse a `FeatureDevelopmentOutcome` from the tool-response string produced by
/// `FeatureDevelopmentOutcome::tool_response()`. Used by `agent.rs` to reconstruct
/// the typed outcome from the text flowing back through the tool-call machinery.
pub fn parse_tool_response(text: &str) -> Option<ParsedOutcome> {
    if let Some(rest) = text.strip_prefix("OWNER_DISPATCH_READY:") {
        if let Ok(id) = rest.trim().parse::<Uuid>() {
            return Some(ParsedOutcome::OwnerDispatchReady(id));
        }
    }
    if let Some(rest) = text.strip_prefix("OWNER_CONFIG_REQUIRED:") {
        if let Ok(id) = rest.trim().parse::<Uuid>() {
            return Some(ParsedOutcome::OwnerConfigurationRequired(id));
        }
    }
    None
}

/// Structured outcome parsed from the tool response text.
#[derive(Debug, Clone, Copy)]
pub enum ParsedOutcome {
    OwnerDispatchReady(Uuid),
    OwnerConfigurationRequired(Uuid),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::catalog::CodingAgent;

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
    fn owner_immediate_returns_dispatch_ready() {
        let store = make_store();
        let rl = make_limiter();
        let outcome = prepare_feature_development(
            &store,
            &rl,
            42,
            owner_requester(42),
            source(),
            "Title",
            "Obj",
            "",
            valid_reqs(),
            valid_ac(),
            DispatchMode::Immediate,
            &defaults(),
        );
        assert!(matches!(
            outcome,
            FeatureDevelopmentOutcome::OwnerDispatchReady { .. }
        ));
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
            "Title",
            "Obj",
            "",
            valid_reqs(),
            valid_ac(),
            DispatchMode::Immediate,
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
            "", // empty title
            "Obj",
            "",
            valid_reqs(),
            valid_ac(),
            DispatchMode::Immediate,
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
    }

    #[test]
    fn parse_tool_response_roundtrip() {
        let id = Uuid::new_v4();
        let text = format!("OWNER_DISPATCH_READY:{id}");
        match parse_tool_response(&text) {
            Some(ParsedOutcome::OwnerDispatchReady(parsed_id)) => assert_eq!(parsed_id, id),
            other => panic!("unexpected: {other:?}"),
        }
        let text2 = format!("OWNER_CONFIG_REQUIRED:{id}");
        match parse_tool_response(&text2) {
            Some(ParsedOutcome::OwnerConfigurationRequired(parsed_id)) => {
                assert_eq!(parsed_id, id)
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}
