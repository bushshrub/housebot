//! Versioned catalog of coding agents and models.
//!
//! The catalog embedded at compile time is the single source of truth for every
//! selectable combination. Neither Rust code nor shell scripts hardcode model lists
//! separately — they all read from this catalog.

use std::collections::HashMap;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// The supported coding agent. Codex and Claude Code were dropped from the
/// rebuild; the enum survives because the catalog is keyed by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodingAgent {
    OpenCode,
}

impl CodingAgent {
    pub fn display_name(self) -> &'static str {
        match self {
            CodingAgent::OpenCode => "OpenCode",
        }
    }

    pub fn id_str(self) -> &'static str {
        match self {
            CodingAgent::OpenCode => "opencode",
        }
    }

    /// The GitHub issue label for this agent.
    pub fn agent_label(self) -> &'static str {
        match self {
            CodingAgent::OpenCode => "agent:opencode",
        }
    }
}

impl std::str::FromStr for CodingAgent {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelDescriptor {
    pub id: String,
    pub display_name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentDescriptor {
    pub display_name: String,
    pub default_model: String,
    pub models: Vec<ModelDescriptor>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CliVersions {
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
        let json = include_str!("../../../.github/agents/catalog.json");
        Self::from_json(json).expect("embedded catalog.json must be valid")
    }

    pub fn models_for(&self, agent: CodingAgent) -> &[ModelDescriptor] {
        self.agents
            .get(&agent)
            .map(|d| d.models.as_slice())
            .unwrap_or(&[])
    }

    /// Validate that agent/model is a known combination and return a `ValidatedAgentSelection`.
    pub fn validate_selection(
        &self,
        agent: CodingAgent,
        model: &str,
    ) -> Result<ValidatedAgentSelection> {
        let agent_desc = self
            .agents
            .get(&agent)
            .ok_or_else(|| anyhow::anyhow!("Unknown agent: {:?}", agent))?;
        agent_desc
            .models
            .iter()
            .find(|m| m.id == model)
            .ok_or_else(|| {
                anyhow::anyhow!("Model '{}' is not configured for agent {:?}", model, agent)
            })?;
        Ok(ValidatedAgentSelection {
            agent,
            model: model.to_string(),
            catalog_revision: self.catalog_revision.clone(),
        })
    }
}

/// A fully validated agent/model combination ready for dispatch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatedAgentSelection {
    pub agent: CodingAgent,
    pub model: String,
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
        let json = r#"{"schema_version":2,"catalog_revision":"x","cli_versions":{"opencode":"1"},"agents":{}}"#;
        assert!(AgentCatalog::from_json(json).is_err());
    }

    #[test]
    fn opencode_is_the_only_agent() {
        let catalog = test_catalog();
        assert_eq!(
            catalog.agents.keys().collect::<Vec<_>>(),
            vec![&CodingAgent::OpenCode]
        );
    }

    /// The catalog is embedded and keyed by `CodingAgent`, so a retired agent
    /// left in the JSON fails deserialization at load and panics the bot on
    /// startup rather than being ignored.
    #[test]
    fn retired_agents_are_rejected_by_the_catalog() {
        let json = r#"{"schema_version":1,"catalog_revision":"x","cli_versions":{"opencode":"1"},"agents":{"codex":{"display_name":"Codex","default_model":"default","models":[]}}}"#;
        assert!(AgentCatalog::from_json(json).is_err());
    }

    #[test]
    fn models_for_returns_slice() {
        let catalog = test_catalog();
        assert!(!catalog.models_for(CodingAgent::OpenCode).is_empty());
    }

    #[test]
    fn validate_selection_succeeds_for_valid_combo() {
        let catalog = test_catalog();
        let models = catalog.models_for(CodingAgent::OpenCode);
        assert!(catalog
            .validate_selection(CodingAgent::OpenCode, &models[0].id)
            .is_ok());
    }

    #[test]
    fn validate_selection_rejects_invalid_model() {
        let catalog = test_catalog();
        let result = catalog.validate_selection(CodingAgent::OpenCode, "gpt-5");
        assert!(result.is_err());
    }

    #[test]
    fn agent_from_str_roundtrip() {
        assert_eq!(
            "opencode".parse::<CodingAgent>().unwrap(),
            CodingAgent::OpenCode
        );
        assert_eq!(CodingAgent::OpenCode.id_str(), "opencode");
    }

    #[test]
    fn unknown_agent_id_returns_error() {
        assert!("gpt".parse::<CodingAgent>().is_err());
        assert!("codex".parse::<CodingAgent>().is_err());
        assert!("claude".parse::<CodingAgent>().is_err());
    }
}
