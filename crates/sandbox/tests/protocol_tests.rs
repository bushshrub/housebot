use housebot_sandbox::protocol::*;

#[test]
fn network_access_serialization() {
    let none = serde_json::to_value(NetworkAccess::None).unwrap();
    assert_eq!(none, serde_json::json!("none"));

    let public = serde_json::to_value(NetworkAccess::PublicInternet).unwrap();
    assert_eq!(public, serde_json::json!("public"));
}

#[test]
fn network_access_deserialization() {
    let none: NetworkAccess = serde_json::from_str("\"none\"").unwrap();
    assert_eq!(none, NetworkAccess::None);

    let public: NetworkAccess = serde_json::from_str("\"public\"").unwrap();
    assert_eq!(public, NetworkAccess::PublicInternet);
}

#[test]
fn sandbox_request_serialization() {
    let req = SandboxRequest::new("start", serde_json::json!({"network": "none"}));
    let json = serde_json::to_value(&req).unwrap();
    assert_eq!(json["method"], "start");
    assert_eq!(json["params"]["network"], "none");
    assert!(json["id"].is_string());
}

#[test]
fn sandbox_response_ok() {
    let resp = SandboxResponse::ok("req-1".into(), serde_json::json!({"sandbox_id": "abc-123"}));
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["id"], "req-1");
    assert_eq!(json["result"]["sandbox_id"], "abc-123");
    assert!(json.get("error").is_none());
}

#[test]
fn sandbox_response_err() {
    let resp = SandboxResponse::err("req-1".into(), "something went wrong".into());
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["id"], "req-1");
    assert_eq!(json["error"], "something went wrong");
    assert!(json.get("result").is_none());
}

#[test]
fn sandbox_response_into_result_ok() {
    let resp = SandboxResponse::ok("1".into(), serde_json::json!("ok"));
    assert_eq!(resp.into_result().unwrap(), "ok");
}

#[test]
fn sandbox_response_into_result_err() {
    let resp = SandboxResponse::err("1".into(), "fail".into());
    assert!(resp.into_result().is_err());
}

#[test]
fn command_result_serialization() {
    let cr = CommandResult {
        exit_code: 0,
        stdout: "hello".into(),
        stderr: String::new(),
        truncated: false,
    };
    let json = serde_json::to_value(&cr).unwrap();
    assert_eq!(json["exit_code"], 0);
    assert_eq!(json["stdout"], "hello");
}

#[test]
fn file_entry_serialization() {
    let entry = FileEntry {
        name: "src/main.rs".into(),
        entry_type: "file".into(),
        size: Some(42),
    };
    let json = serde_json::to_value(&entry).unwrap();
    assert_eq!(json["name"], "src/main.rs");
    assert_eq!(json["type"], "file");
    assert_eq!(json["size"], 42);
}

#[test]
fn search_match_serialization() {
    let m = SearchMatch {
        path: "src/lib.rs".into(),
        line_number: 10,
        line: "fn main()".into(),
    };
    let json = serde_json::to_value(&m).unwrap();
    assert_eq!(json["path"], "src/lib.rs");
    assert_eq!(json["line_number"], 10);
}

#[test]
fn file_contents_serialization() {
    let fc = FileContents {
        contents: "fn main() {}".into(),
        truncated: false,
        binary: false,
        line_count: 1,
    };
    let json = serde_json::to_value(&fc).unwrap();
    assert_eq!(json["line_count"], 1);
    assert!(!json["binary"].as_bool().unwrap());
}

#[test]
fn search_result_truncated_flag() {
    let sr = SearchResult {
        matches: vec![],
        truncated: true,
    };
    let json = serde_json::to_value(&sr).unwrap();
    assert!(json["truncated"].as_bool().unwrap());
}

#[test]
fn sandbox_request_has_unique_ids() {
    let req1 = SandboxRequest::new("test", serde_json::json!({}));
    let req2 = SandboxRequest::new("test", serde_json::json!({}));
    assert_ne!(req1.id, req2.id);
}
