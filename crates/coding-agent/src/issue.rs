//! GitHub issue body and metadata for automated development jobs.

use serde::{Deserialize, Serialize};

use super::catalog::{CodingAgent, ValidatedAgentSelection};
use super::pending::DevelopmentSpecification;

const MAX_ISSUE_BODY: usize = 25_000;

/// Machine-readable metadata embedded as a hidden HTML comment in the issue body.
/// The workflow parses this; the human-readable bullets are for readability only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobMetadata {
    pub schema_version: u32,
    pub agent: CodingAgent,
    pub model: String,
    pub catalog_revision: String,
}

impl JobMetadata {
    pub fn new(selection: &ValidatedAgentSelection) -> Self {
        Self {
            schema_version: 1,
            agent: selection.agent,
            model: selection.model.clone(),
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

/// Legacy comment retained for compatibility with older issue-driven dispatches.
pub const DISPATCH_TRIGGER_COMMENT: &str = "/oc Implement the feature described in this issue. \
     Follow the repository conventions, commit your changes, and open a pull request that \
     closes this issue.";

/// Build the prompt passed as a `workflow_dispatch` input to the
/// `opencode-dispatch` workflow.
fn build_dispatch_prompt(issue_number: u64) -> String {
    format!(
        "Implement the feature described in issue #{issue_number}. \
         Follow the repository conventions, commit your changes, and open a pull request that \
         closes this issue."
    )
}

/// Build the `workflow_dispatch` inputs for a development job.
///
/// Every value is a string: the API returns 422 for non-string values even on
/// inputs the workflow declares as `type: number`.
pub fn dispatch_inputs(
    issue_number: u64,
    model: &str,
    requester_id: u64,
) -> serde_json::Map<String, serde_json::Value> {
    let mut inputs = serde_json::Map::new();
    inputs.insert(
        "issue_number".into(),
        serde_json::Value::String(issue_number.to_string()),
    );
    inputs.insert(
        "prompt".into(),
        serde_json::Value::String(build_dispatch_prompt(issue_number)),
    );
    inputs.insert("model".into(), serde_json::Value::String(model.to_string()));
    inputs.insert(
        "requester_id".into(),
        serde_json::Value::String(requester_id.to_string()),
    );
    inputs
}

/// Return the manually-dispatched workflow for the selected coding agent.
pub fn dispatch_workflow_file(agent: CodingAgent) -> &'static str {
    match agent {
        CodingAgent::OpenCode => "opencode-dispatch.yml",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{AgentCatalog, CodingAgent};

    fn make_selection() -> ValidatedAgentSelection {
        let catalog = AgentCatalog::load_embedded();
        let models = catalog.models_for(CodingAgent::OpenCode);
        catalog
            .validate_selection(CodingAgent::OpenCode, &models[0].id)
            .unwrap()
    }

    fn make_spec() -> DevelopmentSpecification {
        crate::pending::DevelopmentSpecification {
            issue_number: 1,
            title: "Add feature X".into(),
            objective: "Make X work".into(),
            context: "Currently no X".into(),
            requirements: vec!["Implement X".into()],
            acceptance_criteria: vec!["X works".into()],
        }
    }

    /// The dispatch API rejects any input the workflow does not declare, so a
    /// key added here without a matching `inputs:` entry fails only at runtime.
    #[test]
    fn every_dispatch_input_is_declared_by_the_workflow() {
        let workflow = include_str!("../../../.github/workflows/opencode-dispatch.yml");
        let declared: Vec<&str> = workflow
            .lines()
            .filter(|l| l.starts_with("      ") && l.trim_end().ends_with(':'))
            .map(|l| l.trim().trim_end_matches(':'))
            .collect();
        for key in dispatch_inputs(1, "opencode/deepseek-v4-flash-free", 2).keys() {
            assert!(
                declared.contains(&key.as_str()),
                "input '{key}' is not declared in opencode-dispatch.yml"
            );
        }
    }

    #[test]
    fn dispatch_inputs_are_all_strings() {
        let inputs = dispatch_inputs(7, "opencode/big-pickle", 9);
        assert!(inputs.values().all(serde_json::Value::is_string));
        assert_eq!(inputs["model"], "opencode/big-pickle");
        assert_eq!(inputs["issue_number"], "7");
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
    fn dispatch_workflow_matches_agent() {
        assert_eq!(
            dispatch_workflow_file(CodingAgent::OpenCode),
            "opencode-dispatch.yml"
        );
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
