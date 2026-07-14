//! Tool for preparing an automated coding-agent development job.
//!
//! The LLM calls this to build a structured spec. The actual dispatch requires
//! the Discord owner to select an agent, model, and effort level via Discord
//! components, then explicitly confirm.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::coding_agent::pending::{
    DevelopmentSpecification, PendingDevelopmentJob, PendingJobStore,
};
use crate::rate_limit::RateLimiter;

const RATE_LIMIT_MAX: usize = 2;
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(600); // 10 minutes

/// Prefix returned in the agent result to signal the Discord layer to open the
/// component UI for this pending job.
pub const DISPATCH_FLOW_PREFIX: &str = "DISPATCH_FLOW:";

/// OpenAI-style tool definition.
pub fn definition() -> Value {
    json!({
        "name": "prepare_feature_development",
        "description": "Prepare an automated feature-development job for owner review. \
            This does NOT start execution — the Discord owner must select a coding agent, \
            model, effort level, and confirm dispatch. \
            Only call this when the owner explicitly requests automated development. \
            For ordinary feature suggestions, use create_feature_request instead.",
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
                }
            },
            "required": ["title", "objective", "requirements", "acceptance_criteria"]
        }
    })
}

pub fn default_rate_limiter() -> RateLimiter {
    RateLimiter::new(RATE_LIMIT_MAX, RATE_LIMIT_WINDOW)
}

/// Validate inputs, create a `PendingDevelopmentJob`, store it, and return the
/// trigger marker `DISPATCH_FLOW:<uuid>` for the bot to intercept.
///
/// Authorization is enforced here: only the configured owner may call this.
#[allow(clippy::too_many_arguments)]
pub fn prepare_feature_development(
    store: &Arc<PendingJobStore>,
    dispatch_limiter: &RateLimiter,
    owner_id: u64,
    requesting_user_id: &str,
    channel_id: u64,
    title: &str,
    objective: &str,
    context: &str,
    requirements: Vec<String>,
    acceptance_criteria: Vec<String>,
) -> String {
    // Authorization: only the configured owner may prepare development jobs.
    if owner_id == 0 {
        return "Error: OWNER_DISCORD_ID is not configured — automated development is disabled."
            .to_string();
    }
    if requesting_user_id != owner_id.to_string() {
        return "Error: Only the configured bot owner can use the automated development tool."
            .to_string();
    }

    // Rate limiting.
    if dispatch_limiter.check(requesting_user_id) {
        return format!(
            "Error: Rate limit exceeded — at most {RATE_LIMIT_MAX} coding jobs may be dispatched \
             every {} minutes.",
            RATE_LIMIT_WINDOW.as_secs() / 60
        );
    }

    // Input validation.
    if title.is_empty() || title.chars().count() > 100 {
        return "Error: Title must be 1–100 characters.".to_string();
    }
    if objective.is_empty() || objective.chars().count() > 4_000 {
        return "Error: Objective must be 1–4,000 characters.".to_string();
    }
    if context.chars().count() > 8_000 {
        return "Error: Context must not exceed 8,000 characters.".to_string();
    }
    if requirements.is_empty() || requirements.len() > 25 {
        return "Error: Requirements must have 1–25 entries.".to_string();
    }
    if acceptance_criteria.is_empty() || acceptance_criteria.len() > 25 {
        return "Error: Acceptance criteria must have 1–25 entries.".to_string();
    }
    for r in &requirements {
        if r.is_empty() || r.chars().count() > 1_000 {
            return "Error: Each requirement must be 1–1,000 characters.".to_string();
        }
    }
    for a in &acceptance_criteria {
        if a.is_empty() || a.chars().count() > 1_000 {
            return "Error: Each acceptance criterion must be 1–1,000 characters.".to_string();
        }
    }

    let spec = DevelopmentSpecification {
        title: title.to_string(),
        objective: objective.to_string(),
        context: context.to_string(),
        requirements,
        acceptance_criteria,
    };

    let job = PendingDevelopmentJob::new(owner_id, channel_id, spec);
    let id = job.id;
    store.insert(job);

    format!("{DISPATCH_FLOW_PREFIX}{id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_store() -> Arc<PendingJobStore> {
        Arc::new(PendingJobStore::default())
    }

    fn valid_reqs() -> Vec<String> {
        vec!["Do the thing".into()]
    }
    fn valid_ac() -> Vec<String> {
        vec!["Thing is done".into()]
    }

    #[test]
    fn owner_accepted() {
        let store = make_store();
        let rl = default_rate_limiter();
        let result = prepare_feature_development(
            &store,
            &rl,
            42,
            "42",
            1,
            "Title",
            "Obj",
            "",
            valid_reqs(),
            valid_ac(),
        );
        assert!(result.starts_with(DISPATCH_FLOW_PREFIX), "got: {result}");
    }

    #[test]
    fn non_owner_rejected() {
        let store = make_store();
        let rl = default_rate_limiter();
        let result = prepare_feature_development(
            &store,
            &rl,
            42,
            "99",
            1,
            "Title",
            "Obj",
            "",
            valid_reqs(),
            valid_ac(),
        );
        assert!(result.starts_with("Error:"));
        assert!(result.contains("owner"));
    }

    #[test]
    fn missing_owner_id_rejected() {
        let store = make_store();
        let rl = default_rate_limiter();
        let result = prepare_feature_development(
            &store,
            &rl,
            0,
            "42",
            1,
            "Title",
            "Obj",
            "",
            valid_reqs(),
            valid_ac(),
        );
        assert!(result.starts_with("Error:"));
        assert!(result.contains("OWNER_DISCORD_ID"));
    }

    #[test]
    fn empty_title_rejected() {
        let store = make_store();
        let rl = default_rate_limiter();
        let result = prepare_feature_development(
            &store,
            &rl,
            1,
            "1",
            1,
            "",
            "Obj",
            "",
            valid_reqs(),
            valid_ac(),
        );
        assert!(result.starts_with("Error:"));
    }

    #[test]
    fn title_over_100_chars_rejected() {
        let store = make_store();
        let rl = default_rate_limiter();
        let long_title = "a".repeat(101);
        let result = prepare_feature_development(
            &store,
            &rl,
            1,
            "1",
            1,
            &long_title,
            "Obj",
            "",
            valid_reqs(),
            valid_ac(),
        );
        assert!(result.starts_with("Error:"));
    }

    #[test]
    fn empty_requirements_rejected() {
        let store = make_store();
        let rl = default_rate_limiter();
        let result = prepare_feature_development(
            &store,
            &rl,
            1,
            "1",
            1,
            "Title",
            "Obj",
            "",
            vec![],
            valid_ac(),
        );
        assert!(result.starts_with("Error:"));
    }

    #[test]
    fn empty_acceptance_criteria_rejected() {
        let store = make_store();
        let rl = default_rate_limiter();
        let result = prepare_feature_development(
            &store,
            &rl,
            1,
            "1",
            1,
            "Title",
            "Obj",
            "",
            valid_reqs(),
            vec![],
        );
        assert!(result.starts_with("Error:"));
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
}
