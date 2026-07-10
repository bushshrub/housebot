//! Global custom skills — named prompt templates — stored as a single JSON object.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config;
use crate::memory::ensure_dir;

/// A user-defined skill: a named system prompt run against arbitrary input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
}

impl Skill {
    /// Description, falling back to the skill name when absent.
    pub fn description_or_name(&self) -> &str {
        self.description.as_deref().unwrap_or(&self.name)
    }
}

/// Handle to the global skills store.
#[derive(Clone)]
pub struct Skills {
    path: PathBuf,
}

impl Default for Skills {
    fn default() -> Self {
        Self::new(config::data_dir().join("skills.json"))
    }
}

impl Skills {
    /// Create a store backed by the JSON file at `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Load every defined skill, keyed by name.
    pub async fn load_all(&self) -> BTreeMap<String, Skill> {
        let raw = match tokio::fs::read_to_string(&self.path).await {
            Ok(s) => s,
            Err(_) => return BTreeMap::new(),
        };
        if raw.trim().is_empty() {
            return BTreeMap::new();
        }
        serde_json::from_str(&raw).unwrap_or_default()
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
        self.write_all(&all).await
    }

    /// Delete a skill, returning whether it existed.
    pub async fn delete(&self, name: &str) -> std::io::Result<bool> {
        let mut all = self.load_all().await;
        if all.remove(name).is_none() {
            return Ok(false);
        }
        self.write_all(&all).await?;
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
}
