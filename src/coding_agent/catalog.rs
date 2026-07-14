//! Versioned catalog of coding agents, models, and effort levels.
//!
//! The catalog embedded at compile time is the single source of truth for every
//! selectable combination. Neither Rust code nor shell scripts hardcode model lists
//! separately — they all read from this catalog.

use std::collections::HashMap;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// One of the three supported coding agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodingAgent {
    Codex,
    Claude,
    OpenCode,
}

impl CodingAgent {
    pub fn display_name(self) -> &'static str {
        match self {
            CodingAgent::Codex => "Codex",
            CodingAgent::Claude => "Claude Code",
            CodingAgent::OpenCode => "OpenCode",
        }
    }

    pub fn id_str(self) -> &'static str {
        match self {
            CodingAgent::Codex => "codex",
            CodingAgent::Claude => "claude",
            CodingAgent::OpenCode => "opencode",
        }
    }

    /// The GitHub issue label for this agent.
    pub fn agent_label(self) -> &'static str {
        match self {
            CodingAgent::Codex => "agent:codex",
            CodingAgent::Claude => "agent:claude",
            CodingAgent::OpenCode => "agent:opencode",
        }
    }
}

impl std::str::FromStr for CodingAgent {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "codex" => Ok(CodingAgent::Codex),
            "claude" => Ok(CodingAgent::Claude),
            "opencode" => Ok(CodingAgent::OpenCode),
            _ => bail!("Unknown agent id: {s}"),
        }
    }
}

impl std::fmt::Display for CodingAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.display_name())
    }
}

/// How a particular effort level is communicated to the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffortMechanism {
    /// The CLI exposes a direct reasoning/effort flag.
    Native,
    /// Effort is selected by choosing a different model variant.
    Variant,
    /// No native control; bounded by timeout/turn/prompt configuration.
    ExecutionBudget,
}

impl std::fmt::Display for EffortMechanism {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            EffortMechanism::Native => "native",
            EffortMechanism::Variant => "variant",
            EffortMechanism::ExecutionBudget => "execution_budget",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EffortDescriptor {
    pub id: String,
    pub display_name: String,
    pub description: Option<String>,
    pub mechanism: EffortMechanism,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelDescriptor {
    pub id: String,
    pub display_name: String,
    pub description: Option<String>,
    pub default_effort: String,
    pub efforts: Vec<EffortDescriptor>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentDescriptor {
    pub display_name: String,
    pub default_model: String,
    pub models: Vec<ModelDescriptor>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CliVersions {
    pub codex: String,
    pub claude: String,
    pub opencode: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentCatalog {
    pub schema_version: u32,
    pub catalog_revision: String,
    pub cli_versions: CliVersions,
    pub agents: HashMap<CodingAgent, AgentDescriptor>,
}

impl AgentCatalog {
    /// Parse catalog from JSON, validating schema version.
    pub fn from_json(json: &str) -> Result<Self> {
        let catalog: Self = serde_json::from_str(json)?;
        if catalog.schema_version != 1 {
            bail!(
                "Unsupported catalog schema_version {}; only version 1 is supported",
                catalog.schema_version
            );
        }
        Ok(catalog)
    }

    /// The catalog embedded at compile time from `.github/agents/catalog.json`.
    pub fn load_embedded() -> Self {
        let json = include_str!("../../.github/agents/catalog.json");
        Self::from_json(json).expect("embedded catalog.json must be valid")
    }

    pub fn models_for(&self, agent: CodingAgent) -> &[ModelDescriptor] {
        self.agents
            .get(&agent)
            .map(|d| d.models.as_slice())
            .unwrap_or(&[])
    }

    pub fn efforts_for(&self, agent: CodingAgent, model: &str) -> Option<&[EffortDescriptor]> {
        self.agents
            .get(&agent)?
            .models
            .iter()
            .find(|m| m.id == model)
            .map(|m| m.efforts.as_slice())
    }

    /// Validate that agent/model/effort is a known combination and return a `ValidatedAgentSelection`.
    pub fn validate_selection(
        &self,
        agent: CodingAgent,
        model: &str,
        effort: &str,
    ) -> Result<ValidatedAgentSelection> {
        let agent_desc = self
            .agents
            .get(&agent)
            .ok_or_else(|| anyhow::anyhow!("Unknown agent: {:?}", agent))?;
        let model_desc = agent_desc
            .models
            .iter()
            .find(|m| m.id == model)
            .ok_or_else(|| {
                anyhow::anyhow!("Model '{}' is not configured for agent {:?}", model, agent)
            })?;
        let effort_desc = model_desc
            .efforts
            .iter()
            .find(|e| e.id == effort)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Effort '{}' is not valid for model '{}' on agent {:?}",
                    effort,
                    model,
                    agent
                )
            })?;
        Ok(ValidatedAgentSelection {
            agent,
            model: model.to_string(),
            effort: effort.to_string(),
            effort_mechanism: effort_desc.mechanism,
            catalog_revision: self.catalog_revision.clone(),
        })
    }
}

/// A fully validated agent/model/effort combination ready for dispatch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatedAgentSelection {
    pub agent: CodingAgent,
    pub model: String,
    pub effort: String,
    pub effort_mechanism: EffortMechanism,
    pub catalog_revision: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_catalog() -> AgentCatalog {
        AgentCatalog::load_embedded()
    }

    #[test]
    fn embedded_catalog_parses_without_error() {
        let _ = test_catalog();
    }

    #[test]
    fn schema_version_must_be_one() {
        let json = r#"{"schema_version":2,"catalog_revision":"x","cli_versions":{"codex":"1","claude":"1","opencode":"1"},"agents":{}}"#;
        assert!(AgentCatalog::from_json(json).is_err());
    }

    #[test]
    fn all_three_agents_are_present() {
        let catalog = test_catalog();
        for agent in [
            CodingAgent::Codex,
            CodingAgent::Claude,
            CodingAgent::OpenCode,
        ] {
            assert!(
                catalog.agents.contains_key(&agent),
                "Missing agent: {:?}",
                agent
            );
        }
    }

    #[test]
    fn models_for_returns_slice() {
        let catalog = test_catalog();
        assert!(!catalog.models_for(CodingAgent::Claude).is_empty());
        assert!(!catalog.models_for(CodingAgent::Codex).is_empty());
        assert!(!catalog.models_for(CodingAgent::OpenCode).is_empty());
    }

    #[test]
    fn efforts_for_returns_some_for_known_model() {
        let catalog = test_catalog();
        let models = catalog.models_for(CodingAgent::Claude);
        let model_id = &models[0].id;
        assert!(catalog.efforts_for(CodingAgent::Claude, model_id).is_some());
    }

    #[test]
    fn efforts_for_returns_none_for_unknown_model() {
        let catalog = test_catalog();
        assert!(catalog
            .efforts_for(CodingAgent::Claude, "nonexistent-model")
            .is_none());
    }

    #[test]
    fn validate_selection_succeeds_for_valid_combo() {
        let catalog = test_catalog();
        let models = catalog.models_for(CodingAgent::Claude);
        let model = &models[0];
        let effort = &model.efforts[0];
        assert!(catalog
            .validate_selection(CodingAgent::Claude, &model.id, &effort.id)
            .is_ok());
    }

    #[test]
    fn validate_selection_rejects_invalid_effort() {
        let catalog = test_catalog();
        let models = catalog.models_for(CodingAgent::Claude);
        let result = catalog.validate_selection(CodingAgent::Claude, &models[0].id, "ultra");
        assert!(result.is_err());
    }

    #[test]
    fn validate_selection_rejects_invalid_model() {
        let catalog = test_catalog();
        let result = catalog.validate_selection(CodingAgent::Claude, "gpt-5", "high");
        assert!(result.is_err());
    }

    #[test]
    fn agent_from_str_roundtrip() {
        for (id, agent) in [
            ("codex", CodingAgent::Codex),
            ("claude", CodingAgent::Claude),
            ("opencode", CodingAgent::OpenCode),
        ] {
            assert_eq!(id.parse::<CodingAgent>().unwrap(), agent);
            assert_eq!(agent.id_str(), id);
        }
    }

    #[test]
    fn unknown_agent_id_returns_error() {
        assert!("gpt".parse::<CodingAgent>().is_err());
    }
}
