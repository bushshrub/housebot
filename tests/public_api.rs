//! End-to-end tests against the housebot crate's public API surface.

use std::collections::BTreeMap;

use housebot::agent::build_system_prompt;
use housebot::bot::{extract_code_files, split_text};
use housebot::graph_render::{render_png, GraphBuilder};
use housebot::history::History;
use housebot::llm::ThinkingMode;
use housebot::memory::Memory;
use housebot::notes::Notes;
use housebot::skills::{Skill, Skills};
use serde_json::json;
use tempfile::TempDir;

#[tokio::test]
async fn storage_layer_round_trips_across_modules() {
    let dir = TempDir::new().unwrap();
    let memory = Memory::new(dir.path().join("memory"));
    let notes = Notes::new(dir.path().join("notes"));
    let history = History::new(dir.path().join("history"), 30);
    let skills = Skills::new(dir.path().join("skills.json"));

    memory.save("42", "Likes strong tea").await.unwrap();
    notes.save(42, "shopping", "milk, eggs").await.unwrap();
    history
        .append_turn(
            "42",
            json!({"role": "user", "content": "hi"}),
            vec![json!({"role": "assistant", "content": "hello"})],
        )
        .await
        .unwrap();
    skills
        .save(Skill {
            name: "greet".into(),
            description: Some("Say hi".into()),
            prompt: "Greet the user warmly.".into(),
            created_by: Some("42".into()),
            editors: Vec::new(),
        })
        .await
        .unwrap();

    assert_eq!(memory.load("42").await, "Likes strong tea");
    assert_eq!(
        notes.get(42, "shopping").await.as_deref(),
        Some("milk, eggs")
    );
    assert_eq!(history.load("42").await.len(), 2);
    assert_eq!(
        skills.get("greet").await.unwrap().prompt,
        "Greet the user warmly."
    );
}

#[test]
fn system_prompt_reflects_memory_and_skills() {
    let mut skills = BTreeMap::new();
    skills.insert(
        "summarize".into(),
        Skill {
            name: "summarize".into(),
            description: Some("Condense text".into()),
            prompt: "..".into(),
            created_by: None,
            editors: Vec::new(),
        },
    );
    let prompt = build_system_prompt(
        "alice",
        "7",
        "Alice",
        "Ali",
        "Prefers metric units",
        &skills,
        Some("Be terse"),
        true,
    );
    assert!(prompt.contains("Alice"));
    assert!(prompt.contains("Prefers metric units"));
    assert!(prompt.contains("summarize"));
    assert!(prompt.contains("Be terse"));
}

#[test]
fn thinking_mode_is_publicly_parseable() {
    assert_eq!("max".parse::<ThinkingMode>(), Ok(ThinkingMode::Max));
    assert!(ThinkingMode::Low.max_completion_tokens() > 0);
}

#[test]
fn message_splitting_and_code_extraction_are_public() {
    let long = "x".repeat(4500);
    let chunks = split_text(&long, 2000);
    assert!(chunks.iter().all(|c| c.chars().count() <= 2000));
    assert_eq!(chunks.concat().len(), long.len());

    let big_block = format!("Here:\n```python\n{}```", "print(1)\n".repeat(200));
    let (rendered, files) = extract_code_files(&big_block);
    assert_eq!(files.len(), 1);
    assert!(files[0].0.ends_with(".py"));
    assert!(rendered.contains(&files[0].0));
}

#[test]
fn graph_render_is_reachable_through_the_crate() {
    let mut graph = GraphBuilder::default();
    let a = graph.add_node("a", "A");
    let b = graph.add_node("b", "B");
    graph.add_edge(a, b);
    let png = render_png(&graph).unwrap();
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
}
