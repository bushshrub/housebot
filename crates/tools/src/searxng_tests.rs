//! Unit tests for `searxng` (split out to keep the module under 400 lines).

use super::*;

fn response(json: &str) -> SearchResponse {
    serde_json::from_str(json).unwrap()
}

#[test]
fn formats_results_with_title_url_and_snippet() {
    let parsed = response(
        r#"{"results":[{"title":"Rust","url":"https://rust-lang.org","content":"A language","engine":"brave"}]}"#,
    );
    let out = format_results(&parsed, 10);
    assert!(out.contains("Found 1 search results"));
    assert!(out.contains("Rust"));
    assert!(out.contains("https://rust-lang.org"));
    assert!(out.contains("A language"));
    assert!(out.contains("(via brave)"));
}

#[test]
fn respects_result_limit() {
    let parsed = response(
        r#"{"results":[
            {"title":"a","url":"https://a.example"},
            {"title":"b","url":"https://b.example"},
            {"title":"c","url":"https://c.example"}
        ]}"#,
    );
    let out = format_results(&parsed, 2);
    assert!(out.contains("Found 2 search results"));
    assert!(!out.contains("c.example"));
}

#[test]
fn skips_results_without_urls() {
    let parsed =
        response(r#"{"results":[{"title":"nourl"},{"title":"ok","url":"https://ok.example"}]}"#);
    let out = format_results(&parsed, 10);
    assert!(out.contains("Found 1 search results"));
    assert!(!out.contains("nourl"));
}

#[test]
fn empty_results_reports_no_results() {
    let out = format_results(&response(r#"{"results":[]}"#), 10);
    assert!(out.contains("No results"));
}

#[test]
fn includes_string_and_object_answers() {
    let parsed = response(
        r#"{"results":[{"title":"t","url":"https://t.example"}],
            "answers":["42",{"answer":"forty-two","url":"https://a.example"}]}"#,
    );
    let out = format_results(&parsed, 10);
    assert!(out.contains("Answer: 42"));
    assert!(out.contains("Answer: forty-two"));
}

#[test]
fn definition_has_expected_name() {
    assert_eq!(definition()["name"], "web_search");
    assert_eq!(definition()["input_schema"]["required"], json!(["query"]));
}

#[test]
fn deep_research_definition_requires_multiple_questions() {
    let definition = deep_research_definition();
    assert_eq!(definition["name"], "deep_research");
    assert_eq!(
        definition["input_schema"]["properties"]["questions"]["minItems"],
        2
    );
    assert_eq!(
        definition["input_schema"]["properties"]["questions"]["maxItems"],
        5
    );
}

#[test]
fn research_dossier_deduplicates_and_cross_references_sources() {
    let responses = vec![
        (
            "rust overview".to_string(),
            response(
                r#"{"results":[{"title":"Rust","url":"https://rust-lang.org","content":"Overview","engine":"brave"}]}"#,
            ),
        ),
        (
            "rust safety".to_string(),
            response(
                r#"{"results":[
                    {"title":"Rust language","url":"https://rust-lang.org","content":"Memory safety","engine":"google"},
                    {"title":"Rust book","url":"https://doc.rust-lang.org/book/","content":"Official guide"}
                ]}"#,
            ),
        ),
    ];

    let dossier = format_research_dossier("rust", &responses, 5);
    assert_eq!(dossier.matches("URL: https://rust-lang.org").count(), 1);
    assert!(dossier.contains("Appeared in research threads: 1, 2"));
    assert!(dossier.contains("Search engines: brave, google"));
    assert!(dossier.contains("Evidence: Overview"));
    assert!(dossier.contains("Evidence: Memory safety"));
    assert!(
        dossier.find("https://rust-lang.org").unwrap()
            < dossier.find("https://doc.rust-lang.org/book/").unwrap()
    );
}

#[tokio::test]
async fn deep_research_rejects_invalid_plan_before_network_access() {
    let client = SearxNg::from_env();
    let output = client
        .deep_research("rust", &["only one".to_string()], 5, "en")
        .await;
    assert_eq!(
        output,
        "Error: deep research requires between 2 and 5 research questions"
    );
}
