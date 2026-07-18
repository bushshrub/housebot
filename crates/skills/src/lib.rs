use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use housebot_config as config;
use housebot_memory::ensure_dir;

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

    /// Load every defined skill, keyed by name (cached after first load).
    /// Automatically migrates any legacy `prompt`-based skills.
    pub async fn load_all(&self) -> BTreeMap<String, Skill> {
        {
            let cache = self.cache.lock().await;
            if let Some(skills) = &*cache {
                return skills.clone();
            }
        }
        let raw = match tokio::fs::read_to_string(&self.path).await {
            Ok(s) => s,
            Err(_) => return BTreeMap::new(),
        };
        let mut skills: BTreeMap<String, Skill> = if raw.trim().is_empty() {
            BTreeMap::new()
        } else {
            match serde_json::from_str(&raw) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!(
                        target: "housebot::skills",
                        error = %e,
                        path = %self.path.display(),
                        "Failed to parse skills file — returning empty store without caching"
                    );
                    return BTreeMap::new();
                }
            }
        };
        for skill in skills.values_mut() {
            skill.migrate_from_prompt();
        }
        *self.cache.lock().await = Some(skills.clone());
        skills
    }

    async fn write_all(&self, skills: &BTreeMap<String, Skill>) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            ensure_dir(parent).await?;
        }
        let body = serde_json::to_string_pretty(skills).unwrap_or_else(|_| "{}".into());
        tokio::fs::write(&self.path, body).await
    }

    /// Fetch a single skill by name.
    pub async fn get(&self, name: &str) -> Option<Skill> {
        self.load_all().await.remove(name)
    }

    /// Save (or overwrite) a skill under its own name.
    pub async fn save(&self, skill: Skill) -> std::io::Result<()> {
        let mut all = self.load_all().await;
        all.insert(skill.name.clone(), skill);
        self.write_all(&all).await?;
        *self.cache.lock().await = Some(all);
        Ok(())
    }

    /// Delete a skill, returning whether it existed.
    pub async fn delete(&self, name: &str) -> std::io::Result<bool> {
        let mut all = self.load_all().await;
        if all.remove(name).is_none() {
            return Ok(false);
        }
        self.write_all(&all).await?;
        *self.cache.lock().await = Some(all);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store() -> (TempDir, Skills) {
        let tmp = TempDir::new().unwrap();
        let s = Skills::new(tmp.path().join("skills.json"));
        (tmp, s)
    }

    fn skill(name: &str, desc: Option<&str>, instructions: &str) -> Skill {
        Skill {
            name: name.to_string(),
            description: desc.map(String::from),
            instructions: instructions.to_string(),
            triggers: Vec::new(),
            enabled_tools: Vec::new(),
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

    fn full_skill(name: &str) -> Skill {
        Skill {
            name: name.to_string(),
            description: Some("A full-featured skill".into()),
            instructions: "Do the thing".into(),
            triggers: vec![SkillTrigger {
                trigger_type: "keyword".into(),
                value: "standup".into(),
            }],
            enabled_tools: vec!["web_search".into(), "fetch_webpage".into()],
            examples: vec![SkillExample {
                input: "summarize my week".into(),
                output: "Here's your weekly summary...".into(),
            }],
            version: 2,
            version_history: vec![SkillArchive {
                version: 1,
                description: None,
                instructions: "Old instructions".into(),
                triggers: vec![],
                enabled_tools: vec![],
                examples: vec![],
                archived_at: 100,
            }],
            created_by: Some("author".into()),
            editors: vec!["editor1".into()],
            created_at: 50,
            updated_at: 200,
            prompt: None,
        }
    }

    #[tokio::test]
    async fn load_all_empty_when_no_file() {
        let (_t, s) = store();
        assert!(s.load_all().await.is_empty());
    }

    #[tokio::test]
    async fn save_and_load_skill() {
        let (_t, s) = store();
        s.save(skill("greet", Some("Say hello"), "Hello!"))
            .await
            .unwrap();
        let all = s.load_all().await;
        assert_eq!(all.get("greet").unwrap().effective_instructions(), "Hello!");
    }

    #[tokio::test]
    async fn get_existing_skill() {
        let (_t, s) = store();
        s.save(skill("greet", Some("Say hello"), "Hello!"))
            .await
            .unwrap();
        assert_eq!(s.get("greet").await.unwrap().name, "greet");
    }

    #[tokio::test]
    async fn get_missing_returns_none() {
        let (_t, s) = store();
        assert!(s.get("nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn save_overwrites_existing() {
        let (_t, s) = store();
        s.save(skill("greet", Some("old"), "Hi")).await.unwrap();
        s.save(skill("greet", Some("new"), "Hey")).await.unwrap();
        assert_eq!(
            s.get("greet").await.unwrap().description.as_deref(),
            Some("new")
        );
    }

    #[tokio::test]
    async fn delete_existing_skill() {
        let (_t, s) = store();
        s.save(skill("greet", Some("Say hello"), "Hello!"))
            .await
            .unwrap();
        assert!(s.delete("greet").await.unwrap());
        assert!(s.get("greet").await.is_none());
    }

    #[tokio::test]
    async fn delete_missing_returns_false() {
        let (_t, s) = store();
        assert!(!s.delete("nonexistent").await.unwrap());
    }

    #[tokio::test]
    async fn multiple_skills_coexist() {
        let (_t, s) = store();
        s.save(skill("a", None, "A instructions")).await.unwrap();
        s.save(skill("b", None, "B instructions")).await.unwrap();
        let all = s.load_all().await;
        assert!(all.contains_key("a"));
        assert!(all.contains_key("b"));
    }

    #[test]
    fn skill_without_description_uses_name() {
        let sk = skill("a", None, "A instructions");
        assert_eq!(sk.description_or_name(), "a");
    }

    #[test]
    fn effective_instructions_falls_back_to_legacy_prompt() {
        let mut sk = skill("x", None, "");
        sk.prompt = Some("legacy prompt".into());
        assert_eq!(sk.effective_instructions(), "legacy prompt");
    }

    #[test]
    fn effective_instructions_prefers_instructions_over_prompt() {
        let mut sk = skill("x", None, "new instructions");
        sk.prompt = Some("old prompt".into());
        assert_eq!(sk.effective_instructions(), "new instructions");
    }

    #[test]
    fn migrate_from_prompt_moves_to_instructions() {
        let mut sk = Skill {
            name: "x".into(),
            description: None,
            instructions: String::new(),
            triggers: Vec::new(),
            enabled_tools: Vec::new(),
            examples: Vec::new(),
            version: 1,
            version_history: Vec::new(),
            created_by: None,
            editors: Vec::new(),
            created_at: 0,
            updated_at: 0,
            prompt: Some("legacy".into()),
        };
        sk.migrate_from_prompt();
        assert_eq!(sk.instructions, "legacy");
        assert!(sk.prompt.is_none());
    }

    #[test]
    fn bump_version_archives_and_increments() {
        let mut sk = skill("x", None, "v1 instructions");
        sk.description = Some("v1 desc".into());
        sk.triggers = vec![SkillTrigger {
            trigger_type: "keyword".into(),
            value: "test".into(),
        }];
        sk.enabled_tools = vec!["search".into()];
        assert_eq!(sk.version, 1);
        sk.bump_version();
        assert_eq!(sk.version, 2);
        assert_eq!(sk.version_history.len(), 1);
        assert_eq!(sk.version_history[0].version, 1);
        assert_eq!(sk.version_history[0].instructions, "v1 instructions");
        assert_eq!(
            sk.version_history[0].description.as_deref(),
            Some("v1 desc")
        );
        assert_eq!(sk.version_history[0].triggers.len(), 1);
        assert_eq!(sk.version_history[0].enabled_tools, vec!["search"]);
    }

    #[test]
    fn full_skill_round_trip() {
        let sk = full_skill("test_skill");
        assert_eq!(sk.name, "test_skill");
        assert_eq!(sk.triggers.len(), 1);
        assert_eq!(sk.enabled_tools.len(), 2);
        assert_eq!(sk.examples.len(), 1);
        assert_eq!(sk.version, 2);
        assert!(sk.has_triggers());
    }

    #[test]
    fn has_triggers_false_when_empty() {
        let sk = skill("x", None, "instructions");
        assert!(!sk.has_triggers());
    }

    // ── permission tests ─────────────────────────────────────────────────
    fn authored_skill(author: &str) -> Skill {
        Skill {
            name: "x".into(),
            description: None,
            instructions: "p".into(),
            triggers: Vec::new(),
            enabled_tools: Vec::new(),
            examples: Vec::new(),
            version: 1,
            version_history: Vec::new(),
            created_by: Some(author.to_string()),
            editors: vec!["300".into(), "400".into()],
            created_at: 0,
            updated_at: 0,
            prompt: None,
        }
    }

    #[test]
    fn is_author_matches() {
        let sk = authored_skill("100");
        assert!(sk.is_author("100"));
        assert!(!sk.is_author("200"));
    }

    #[test]
    fn can_edit_author_or_editor() {
        let sk = authored_skill("100");
        assert!(sk.can_edit("100"));
        assert!(sk.can_edit("300"));
        assert!(sk.can_edit("400"));
        assert!(!sk.can_edit("500"));
    }

    #[test]
    fn can_edit_author_when_no_created_by() {
        let sk = Skill {
            name: "x".into(),
            description: None,
            instructions: "p".into(),
            triggers: Vec::new(),
            enabled_tools: Vec::new(),
            examples: Vec::new(),
            version: 1,
            version_history: Vec::new(),
            created_by: None,
            editors: vec![],
            created_at: 0,
            updated_at: 0,
            prompt: None,
        };
        assert!(!sk.can_edit("100"));
    }

    #[test]
    fn add_editor_duplicate() {
        let mut sk = authored_skill("100");
        assert!(!sk.add_editor("300"));
        assert_eq!(sk.editors.len(), 2);
    }

    #[test]
    fn add_editor_new() {
        let mut sk = authored_skill("100");
        assert!(sk.add_editor("500"));
        assert!(sk.editors.contains(&"500".to_string()));
    }

    #[test]
    fn remove_editor_present() {
        let mut sk = authored_skill("100");
        assert!(sk.remove_editor("300"));
        assert!(!sk.editors.contains(&"300".to_string()));
    }

    #[test]
    fn remove_editor_missing() {
        let mut sk = authored_skill("100");
        assert!(!sk.remove_editor("999"));
    }

    #[tokio::test]
    async fn legacy_prompt_is_migrated_on_load() {
        let (_t, s) = store();
        // Write old-format JSON with `prompt` field
        let old_json = r#"{"greet":{"name":"greet","description":"old","prompt":"Hello!"}}"#;
        tokio::fs::write(&s.path, old_json).await.unwrap();
        let all = s.load_all().await;
        let skill = all.get("greet").unwrap();
        assert_eq!(skill.effective_instructions(), "Hello!");
        assert_eq!(skill.instructions, "Hello!");
        // prompt should be None after migration
        assert!(skill.prompt.is_none());
        // migrated legacy skills should be at version 1
        assert_eq!(skill.version, 1);
    }

    #[tokio::test]
    async fn new_skills_dont_write_prompt_field() {
        let (_t, s) = store();
        s.save(skill("new_skill", None, "new instructions"))
            .await
            .unwrap();
        let raw = tokio::fs::read_to_string(&s.path).await.unwrap();
        assert!(!raw.contains("\"prompt\""));
        assert!(raw.contains("\"instructions\""));
    }
}
