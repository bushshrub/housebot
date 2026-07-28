//! Shared fixtures for the `bot_tests` tests.

//! Unit tests for `bot` (split out to keep the module under 600 lines).

use super::*;
use tempfile::TempDir;

pub(crate) fn stores() -> (TempDir, Skills, Notes, Memory, History) {
    let tmp = TempDir::new().unwrap();
    let skills = Skills::new(tmp.path().join("skills.json"));
    let notes = Notes::new(tmp.path().join("notes"));
    let memory = Memory::new(tmp.path().join("memories"));
    let history = History::new(tmp.path().join("history"), 30);
    (tmp, skills, notes, memory, history)
}

pub(crate) fn test_skill(name: &str, author: &str) -> crate::skills::Skill {
    crate::skills::Skill {
        name: name.to_string(),
        description: Some("You greet people".to_string()),
        instructions: "You greet people".to_string(),
        triggers: Vec::new(),
        enabled_tools: Vec::new(),
        examples: Vec::new(),
        version: 1,
        version_history: Vec::new(),
        created_by: Some(author.to_string()),
        editors: Vec::new(),
        created_at: 0,
        updated_at: 0,
        prompt: None,
    }
}
