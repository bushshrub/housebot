//! Global custom skills — named prompt templates — stored as a single JSON object.
//!
//! Skills are globally shared. The author owns their skill; only the author (or
//! delegated editors) can modify or delete it.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::config;
use crate::memory::ensure_dir;

/// A user-defined skill: a named system prompt run against arbitrary input.
///
/// Skills are globally visible and executable by anyone. Editing and deletion
/// are restricted to the [`author`](Skill::author_id) and any
/// [`editors`](Skill::editors) they have delegated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub prompt: String,
    /// Discord user ID of the skill's author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    /// Discord user IDs of delegated editors (in addition to the author).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub editors: Vec<String>,
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
        let skills: BTreeMap<String, Skill> = if raw.trim().is_empty() {
            BTreeMap::new()
        } else {
            serde_json::from_str(&raw).unwrap_or_default()
        };
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

    fn skill(name: &str, desc: Option<&str>, prompt: &str) -> Skill {
        Skill {
            name: name.to_string(),
            description: desc.map(String::from),
            prompt: prompt.to_string(),
            created_by: None,
            editors: Vec::new(),
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
        assert_eq!(all.get("greet").unwrap().prompt, "Hello!");
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
        s.save(skill("a", None, "A prompt")).await.unwrap();
        s.save(skill("b", None, "B prompt")).await.unwrap();
        let all = s.load_all().await;
        assert!(all.contains_key("a"));
        assert!(all.contains_key("b"));
    }

    #[tokio::test]
    async fn skill_without_description_uses_name() {
        let sk = skill("a", None, "A prompt");
        assert_eq!(sk.description_or_name(), "a");
    }

    // ── permission tests ─────────────────────────────────────────────────
    fn authored_skill(author: &str) -> Skill {
        Skill {
            name: "x".into(),
            description: None,
            prompt: "p".into(),
            created_by: Some(author.to_string()),
            editors: vec!["300".into(), "400".into()],
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
        assert!(sk.can_edit("100")); // author
        assert!(sk.can_edit("300")); // delegated editor
        assert!(sk.can_edit("400")); // delegated editor
        assert!(!sk.can_edit("500")); // nobody
    }

    #[test]
    fn can_edit_author_when_no_created_by() {
        let sk = Skill {
            name: "x".into(),
            description: None,
            prompt: "p".into(),
            created_by: None,
            editors: vec![],
        };
        assert!(!sk.can_edit("100"));
    }

    #[test]
    fn add_editor_duplicate() {
        let mut sk = authored_skill("100");
        assert!(!sk.add_editor("300")); // already present
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
}
