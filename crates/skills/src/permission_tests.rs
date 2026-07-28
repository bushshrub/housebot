//! Authorship and editor-permission tests for `skills`.

use super::*;

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
