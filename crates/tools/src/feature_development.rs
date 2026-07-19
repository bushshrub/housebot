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

use housebot_coding_agent::pending::{
    DevelopmentRequester, DevelopmentSpecification, DiscordMessageRef, DispatchStage,
    PartialAgentSelection, PendingDevelopmentJob, PendingJobStore,
};
use housebot_config as config;
use housebot_rate_limit::RateLimiter;

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
                "issue_number": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Existing GitHub issue number to implement."
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
            "required": ["issue_number", "title", "objective", "requirements", "acceptance_criteria"]
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
    issue_number: u64,
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
        issue_number,
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

#[cfg(test)]
#[path = "feature_development_tests.rs"]
mod tests;
