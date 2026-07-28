use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use housebot_config as config;
use housebot_memory::ensure_dir;

pub const SKILL_CREATOR_NAME: &str = "skill_creator";

/// A trigger condition that determines when a skill should be activated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillTrigger {
    pub trigger_type: String,
    pub value: String,
}

/// A few-shot example pair for a skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillExample {
    pub input: String,
    pub output: String,
}

/// An archived version of a skill's core configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillArchive {
    pub version: u32,
    pub description: Option<String>,
    pub instructions: String,
    pub triggers: Vec<SkillTrigger>,
    pub enabled_tools: Vec<String>,
    pub examples: Vec<SkillExample>,
    pub archived_at: u64,
}

/// A user-defined skill — a packaged unit of capability with trigger
/// conditions, instructions, tool integration, few-shot examples, and
/// version history.
///
/// Skills are globally visible and executable by anyone. Editing and
/// deletion are restricted to the author and any delegated editors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Core behavioral instructions (replaces the legacy `prompt` field).
    #[serde(default)]
    pub instructions: String,
    /// Conditions that determine when this skill should be activated.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<SkillTrigger>,
    /// Tool names the skill is authorized to use during execution.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enabled_tools: Vec<String>,
    /// Few-shot input/output examples.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<SkillExample>,
    /// Current version number (increments on each modification).
    #[serde(default)]
    pub version: u32,
    /// Archived previous versions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub version_history: Vec<SkillArchive>,
    /// Discord user ID of the skill's author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    /// Discord user IDs of delegated editors.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub editors: Vec<String>,
    /// Unix timestamp of creation.
    #[serde(default)]
    pub created_at: u64,
    /// Unix timestamp of last modification.
    #[serde(default)]
    pub updated_at: u64,
    /// Deprecated: migrated to `instructions` on load.
    #[serde(default, skip_serializing)]
    pub prompt: Option<String>,
}

impl Skill {
    /// Description, falling back to the skill name when absent.
    pub fn description_or_name(&self) -> &str {
        self.description.as_deref().unwrap_or(&self.name)
    }

    /// Whether `user_id` is the original author of this skill.
    pub fn is_author(&self, user_id: &str) -> bool {
        self.created_by.as_deref() == Some(user_id)
    }

    /// Whether `user_id` may edit or delete this skill.
    pub fn can_edit(&self, user_id: &str) -> bool {
        self.is_author(user_id) || self.editors.iter().any(|e| e == user_id)
    }

    /// Add a delegated editor. Returns `false` if already present.
    pub fn add_editor(&mut self, editor_id: &str) -> bool {
        if self.editors.iter().any(|e| e == editor_id) {
            false
        } else {
            self.editors.push(editor_id.to_string());
            true
        }
    }

    /// Remove a delegated editor. Returns `false` if not found.
    pub fn remove_editor(&mut self, editor_id: &str) -> bool {
        let before = self.editors.len();
        self.editors.retain(|e| e != editor_id);
        self.editors.len() < before
    }

    /// Return the effective instructions, falling back to the legacy `prompt`
    /// field when `instructions` is empty (backward compatibility).
    pub fn effective_instructions(&self) -> &str {
        if !self.instructions.is_empty() {
            &self.instructions
        } else if let Some(ref prompt) = self.prompt {
            prompt
        } else {
            ""
        }
    }

    /// Migrate the legacy `prompt` field into `instructions` if instructions
    /// is empty.  Safe to call multiple times.
    pub fn migrate_from_prompt(&mut self) {
        if self.instructions.is_empty() {
            if let Some(prompt) = self.prompt.take() {
                self.instructions = prompt;
                if self.version == 0 {
                    self.version = 1;
                }
            }
        }
    }

    /// Archive the current version's configuration, increment the version,
    /// and set `updated_at` to now.
    pub fn bump_version(&mut self) {
        self.version_history.push(SkillArchive {
            version: self.version,
            description: self.description.clone(),
            instructions: self.instructions.clone(),
            triggers: self.triggers.clone(),
            enabled_tools: self.enabled_tools.clone(),
            examples: self.examples.clone(),
            archived_at: now_secs(),
        });
        self.version += 1;
        self.updated_at = now_secs();
    }

    /// Check whether the skill has any trigger conditions defined.
    pub fn has_triggers(&self) -> bool {
        !self.triggers.is_empty()
    }

    /// Whether this skill's triggers fire for `message`.
    ///
    /// Only deterministically matchable trigger types participate: `always`
    /// always fires and `keyword` fires on a case-insensitive substring match.
    /// `intent` and `context` are advisory — they are surfaced to the agent for
    /// its own judgement rather than matched here.
    pub fn matches_message(&self, message: &str) -> bool {
        let lower = message.to_lowercase();
        self.triggers.iter().any(|t| match t.trigger_type.as_str() {
            "always" => true,
            "keyword" => lower.contains(&t.value.to_lowercase()),
            _ => false,
        })
    }
}

fn builtin_skill_creator() -> Skill {
    Skill {
        name: SKILL_CREATOR_NAME.to_string(),
        description: Some(
            "Design clear, reusable Housebot skills through a review-first workflow.".to_string(),
        ),
        instructions: "Help the user design or improve a Housebot skill. First clarify the \
            desired behavior, boundaries, trigger conditions, and tools it genuinely needs. \
            Prefer focused instructions over broad personality prompts. Use keyword triggers only \
            for precise phrases; use intent or context triggers when literal matching would be \
            brittle. Recommend only tools that actually exist. Add a small number of examples \
            when they materially disambiguate behavior. Check the marketplace before choosing a \
            name or duplicating an existing skill. Present a concise final draft containing the \
            name, description, instructions, triggers, recommended tools, and examples. Obtain \
            explicit user approval before calling create_skill or edit_skill."
            .to_string(),
        triggers: vec![
            SkillTrigger {
                trigger_type: "intent".to_string(),
                value: "create, design, improve, or review a custom skill".to_string(),
            },
            SkillTrigger {
                trigger_type: "keyword".to_string(),
                value: "skill creator".to_string(),
            },
        ],
        enabled_tools: vec![
            "list_skills".to_string(),
            "skill_info".to_string(),
            "create_skill".to_string(),
            "edit_skill".to_string(),
        ],
        examples: Vec::new(),
        version: 1,
        version_history: Vec::new(),
        created_by: None,
        editors: Vec::new(),
        created_at: 0,
        updated_at: 0,
        prompt: None,
    }
}

fn builtin_skills() -> BTreeMap<String, Skill> {
    BTreeMap::from([(SKILL_CREATOR_NAME.to_string(), builtin_skill_creator())])
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Handle to the global skills store.
#[derive(Clone)]
pub struct Skills {
    path: PathBuf,
    cache: Arc<Mutex<Option<BTreeMap<String, Skill>>>>,
}

impl Default for Skills {
    fn default() -> Self {
        Self::new(config::data_dir().join("skills.json"))
    }
}

impl Skills {
    /// Create a store backed by the JSON file at `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            cache: Arc::new(Mutex::new(None)),
        }
    }

    async fn load_from_disk(&self) -> std::io::Result<BTreeMap<String, Skill>> {
        {
            let cache = self.cache.lock().await;
            if let Some(skills) = &*cache {
                return Ok(skills.clone());
            }
        }
        let raw = match tokio::fs::read_to_string(&self.path).await {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error),
        };
        let mut skills: BTreeMap<String, Skill> = if raw.trim().is_empty() {
            BTreeMap::new()
        } else {
            serde_json::from_str(&raw)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?
        };
        for skill in skills.values_mut() {
            skill.migrate_from_prompt();
        }
        skills.insert(SKILL_CREATOR_NAME.to_string(), builtin_skill_creator());
        *self.cache.lock().await = Some(skills.clone());
        Ok(skills)
    }

    /// Load every defined skill, keyed by name (cached after first load).
    /// Automatically migrates any legacy `prompt`-based skills.
    pub async fn load_all(&self) -> BTreeMap<String, Skill> {
        match self.load_from_disk().await {
            Ok(skills) => skills,
            Err(error) => {
                tracing::error!(
                    target: "housebot::skills",
                    %error,
                    path = %self.path.display(),
                    "Failed to load skills file — returning built-ins without caching"
                );
                builtin_skills()
            }
        }
    }

    async fn write_all(&self, skills: &BTreeMap<String, Skill>) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            ensure_dir(parent).await?;
        }
        let persisted: BTreeMap<_, _> = skills
            .iter()
            .filter(|(name, _)| name.as_str() != SKILL_CREATOR_NAME)
            .map(|(name, skill)| (name.clone(), skill.clone()))
            .collect();
        let body = serde_json::to_string_pretty(&persisted).unwrap_or_else(|_| "{}".into());
        tokio::fs::write(&self.path, body).await
    }

    /// Fetch a single skill by name.
    pub async fn get(&self, name: &str) -> Option<Skill> {
        self.load_all().await.remove(name)
    }

    /// Save (or overwrite) a skill under its own name.
    pub async fn save(&self, skill: Skill) -> std::io::Result<()> {
        if skill.name == SKILL_CREATOR_NAME {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "built-in skills cannot be overwritten",
            ));
        }
        let mut all = self.load_from_disk().await?;
        all.insert(skill.name.clone(), skill);
        self.write_all(&all).await?;
        *self.cache.lock().await = Some(all);
        Ok(())
    }

    /// Delete a skill, returning whether it existed.
    pub async fn delete(&self, name: &str) -> std::io::Result<bool> {
        if name == SKILL_CREATOR_NAME {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "built-in skills cannot be deleted",
            ));
        }
        let mut all = self.load_from_disk().await?;
        if all.remove(name).is_none() {
            return Ok(false);
        }
        self.write_all(&all).await?;
        *self.cache.lock().await = Some(all);
        Ok(true)
    }
}

#[cfg(test)]
#[path = "permission_tests.rs"]
mod permission_tests;
#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
