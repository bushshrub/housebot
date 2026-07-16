//! GitHub issue body and metadata for automated development jobs.

use serde::{Deserialize, Serialize};

use super::catalog::{CodingAgent, EffortMechanism, ValidatedAgentSelection};
use super::pending::DevelopmentSpecification;

const MAX_ISSUE_BODY: usize = 25_000;

/// Machine-readable metadata embedded as a hidden HTML comment in the issue body.
/// The workflow parses this; the human-readable bullets are for readability only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobMetadata {
    pub schema_version: u32,
    pub agent: CodingAgent,
    pub model: String,
    pub effort: String,
    pub effort_mechanism: EffortMechanism,
    pub catalog_revision: String,
}

impl JobMetadata {
    pub fn new(selection: &ValidatedAgentSelection) -> Self {
        Self {
            schema_version: 1,
            agent: selection.agent,
            model: selection.model.clone(),
            effort: selection.effort.clone(),
            effort_mechanism: selection.effort_mechanism,
            catalog_revision: selection.catalog_revision.clone(),
        }
    }
}

/// Build the structured GitHub issue body for a development job.
///
/// `requester_display` / `requester_id` identify the original submitter.
/// `approver_display` / `approver_id` identify who authorized execution (often the owner).
/// For owner-direct jobs these pairs are identical.
pub fn build_issue_body(
    spec: &DevelopmentSpecification,
    selection: &ValidatedAgentSelection,
    requester_display: &str,
    requester_id: u64,
    approver_display: &str,
    approver_id: u64,
) -> Result<String, String> {
    let requirements = spec
        .requirements
        .iter()
        .map(|r| format!("- {r}"))
        .collect::<Vec<_>>()
        .join("\n");
    let acceptance = spec
        .acceptance_criteria
        .iter()
        .map(|a| format!("- {a}"))
        .collect::<Vec<_>>()
        .join("\n");

    let metadata = JobMetadata::new(selection);
    let metadata_json = serde_json::to_string_pretty(&metadata)
        .map_err(|e| format!("Failed to serialize metadata: {e}"))?;

    let context_section = if spec.context.trim().is_empty() {
        String::new()
    } else {
        format!("## Context\n{}\n", spec.context.trim())
    };

    let body = format!(
        "## Objective\n{objective}\n\n\
         {context_section}\
         ## Requirements\n{requirements}\n\n\
         ## Acceptance Criteria\n{acceptance}\n\n\
         ## Constraints\n\
         - Keep changes scoped to this issue.\n\
         - Do not merge or deploy.\n\
         - Preserve existing behavior unless explicitly changed.\n\
         - Add or update tests.\n\
         - Run the repository validation suite.\n\n\
         ## Agent Configuration\n\
         - Agent: {agent_display}\n\
         - Model: `{model}`\n\
         - Effort: `{effort}`\n\
         - Effort mechanism: `{mechanism}`\n\
         - Catalog revision: `{revision}`\n\n\
         ## Request Metadata\n\
         - Requested by: `{requester_display}`\n\
         - Requester Discord ID: `{requester_id}`\n\
         - Approved by: `{approver_display}`\n\
         - Approver Discord ID: `{approver_id}`\n\n\
         <!-- housebot-development-job\n\
         {metadata_json}\n\
         -->",
        objective = spec.objective.trim(),
        agent_display = selection.agent.display_name(),
        model = selection.model,
        effort = selection.effort,
        mechanism = selection.effort_mechanism,
        revision = selection.catalog_revision,
    );

    if body.len() > MAX_ISSUE_BODY {
        return Err(format!(
            "Generated issue body is too long ({} chars, limit {})",
            body.len(),
            MAX_ISSUE_BODY
        ));
    }

    Ok(body)
}

/// The issue comment that triggers the standard opencode GitHub workflow
/// (`.github/workflows/opencode.yml`) after the issue is created.
pub const DISPATCH_TRIGGER_COMMENT: &str = "/oc Implement the feature described in this issue. \
     Follow the repository conventions, commit your changes, and open a pull request that \
     closes this issue.";

/// The labels to apply when dispatching (enhancement + queue + agent + source).
pub fn dispatch_labels(agent: CodingAgent) -> Vec<String> {
    vec![
        "enhancement".into(),
        "agent:queued".into(),
        agent.agent_label().into(),
        "source:discord".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::catalog::{AgentCatalog, CodingAgent};

    fn make_selection() -> ValidatedAgentSelection {
        let catalog = AgentCatalog::load_embedded();
        let models = catalog.models_for(CodingAgent::Claude);
        let model = &models[0];
        let effort = &model.efforts[0];
        catalog
            .validate_selection(CodingAgent::Claude, &model.id, &effort.id)
            .unwrap()
    }

    fn make_spec() -> DevelopmentSpecification {
        crate::coding_agent::pending::DevelopmentSpecification {
            title: "Add feature X".into(),
            objective: "Make X work".into(),
            context: "Currently no X".into(),
            requirements: vec!["Implement X".into()],
            acceptance_criteria: vec!["X works".into()],
        }
    }

    #[test]
    fn build_issue_body_contains_objective() {
        let sel = make_selection();
        let spec = make_spec();
        let body = build_issue_body(&spec, &sel, "testuser", 12345, "owner", 1).unwrap();
        assert!(body.contains("Make X work"));
        assert!(body.contains("Implement X"));
        assert!(body.contains("X works"));
    }

    #[test]
    fn build_issue_body_contains_machine_metadata() {
        let sel = make_selection();
        let spec = make_spec();
        let body = build_issue_body(&spec, &sel, "testuser", 12345, "owner", 1).unwrap();
        assert!(body.contains("housebot-development-job"));
        assert!(body.contains("schema_version"));
        assert!(body.contains("catalog_revision"));
    }

    #[test]
    fn build_issue_body_contains_requester_and_approver() {
        let sel = make_selection();
        let spec = make_spec();
        let body = build_issue_body(&spec, &sel, "alice", 111, "owner", 1).unwrap();
        assert!(body.contains("alice"));
        assert!(body.contains("owner"));
        assert!(body.contains("111"));
        assert!(body.contains("Requested by"));
        assert!(body.contains("Approved by"));
    }

    #[test]
    fn dispatch_labels_contains_exactly_one_agent_label() {
        let labels = dispatch_labels(CodingAgent::Claude);
        let agent_labels: Vec<_> = labels.iter().filter(|l| l.starts_with("agent:")).collect();
        // agent:queued and agent:claude
        assert_eq!(agent_labels.len(), 2);
        assert!(labels.contains(&"agent:claude".to_string()));
        assert!(labels.contains(&"agent:queued".to_string()));
    }

    #[test]
    fn dispatch_labels_contains_source_discord() {
        let labels = dispatch_labels(CodingAgent::OpenCode);
        assert!(labels.contains(&"source:discord".to_string()));
    }

    #[test]
    fn metadata_serialization_roundtrip() {
        let sel = make_selection();
        let meta = JobMetadata::new(&sel);
        let json = serde_json::to_string(&meta).unwrap();
        let back: JobMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(back.schema_version, 1);
        assert_eq!(back.model, sel.model);
    }
}
