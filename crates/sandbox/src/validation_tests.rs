//! Unit tests for `validation` (split out to keep the module under 400 lines).

use super::*;

// ── Repository URL validation ─────────────────────────────────────────

#[test]
fn accepts_public_https_github_url() {
    assert!(validate_repository_url("https://github.com/user/repo").is_ok());
}

#[test]
fn accepts_https_url_with_dot_git_suffix() {
    assert!(validate_repository_url("https://github.com/user/repo.git").is_ok());
}

#[test]
fn accepts_https_gitlab_url() {
    assert!(validate_repository_url("https://gitlab.com/user/project").is_ok());
}

#[test]
fn accepts_https_url_with_path_segments() {
    assert!(validate_repository_url("https://github.com/user/repo/tree/main").is_ok());
}

#[test]
fn rejects_ssh_url() {
    let result = validate_repository_url("git@github.com:user/repo.git");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("scheme") || err.contains("URL") || err.contains("Invalid"),
        "got: {err}"
    );
}

#[test]
fn rejects_http_url() {
    let result = validate_repository_url("http://github.com/user/repo");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("scheme"));
}

#[test]
fn rejects_embedded_credentials() {
    let result = validate_repository_url("https://user:pass@github.com/repo");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("credentials"));
}

#[test]
fn rejects_localhost() {
    let result = validate_repository_url("https://localhost/repo");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("private"));
}

#[test]
fn rejects_private_ip() {
    let result = validate_repository_url("https://192.168.1.1/repo");
    assert!(result.is_err());
}

#[test]
fn rejects_file_url() {
    let result = validate_repository_url("file:///etc/passwd");
    assert!(result.is_err());
    // scheme error comes from url::Url parsing for file://
}

#[test]
fn rejects_malformed_url() {
    assert!(validate_repository_url("not a url").is_err());
}

#[test]
fn rejects_empty_url() {
    assert!(validate_repository_url("").is_err());
}

#[test]
fn rejects_url_with_loopback_ip() {
    assert!(validate_repository_url("https://127.0.0.1/repo").is_err());
}

#[test]
fn rejects_url_with_zero_ip() {
    assert!(validate_repository_url("https://0.0.0.0/repo").is_err());
}

#[test]
fn rejects_url_ending_local() {
    assert!(validate_repository_url("https://host.local/repo").is_err());
}

#[test]
fn rejects_url_host_docker_internal() {
    assert!(validate_repository_url("https://host.docker.internal/repo").is_err());
}

// ── Workspace path validation ─────────────────────────────────────────

#[test]
fn accepts_normal_relative_path() {
    assert!(validate_workspace_path("src/main.rs").is_ok());
}

#[test]
fn accepts_nested_relative_path() {
    assert!(validate_workspace_path("src/lib/foo.rs").is_ok());
}

#[test]
fn rejects_absolute_path() {
    assert!(validate_workspace_path("/etc/passwd").is_err());
}

#[test]
fn rejects_parent_dir_escape() {
    assert!(validate_workspace_path("../outside").is_err());
}

#[test]
fn rejects_deep_parent_escape() {
    assert!(validate_workspace_path("src/../../outside").is_err());
}

#[test]
fn accepts_path_with_dot_prefix() {
    // A path component starting with '.' that isn't '..' is fine (e.g. ".hidden")
    assert!(validate_workspace_path("src/.hidden").is_ok());
}

#[test]
fn rejects_empty_path() {
    assert!(validate_workspace_path("").is_err());
}

#[test]
fn rejects_null_byte_path() {
    assert!(validate_workspace_path("src\0/main.rs").is_err());
}

#[test]
fn rejects_deeply_nested_path() {
    let deep = (0..70).map(|_| "a").collect::<Vec<_>>().join("/");
    assert!(validate_workspace_path(&deep).is_err());
}

// ── Query validation ──────────────────────────────────────────────────

#[test]
fn accepts_normal_query() {
    assert!(validate_query("fn main").is_ok());
}

#[test]
fn rejects_empty_query() {
    assert!(validate_query("").is_err());
}

#[test]
fn rejects_null_query() {
    assert!(validate_query("hello\0world").is_err());
}

#[test]
fn rejects_oversized_query() {
    let long = "a".repeat(600);
    assert!(validate_query(&long).is_err());
}

// ── Command validation ────────────────────────────────────────────────

#[test]
fn accepts_normal_command() {
    assert!(validate_command("ls -la").is_ok());
}

#[test]
fn rejects_empty_command() {
    assert!(validate_command("").is_err());
}

#[test]
fn rejects_null_command() {
    assert!(validate_command("echo\0hello").is_err());
}

#[test]
fn rejects_oversized_command() {
    let long = "a".repeat(5000);
    assert!(validate_command(&long).is_err());
}

// ── Branch validation ─────────────────────────────────────────────────

#[test]
fn accepts_normal_branch() {
    assert!(validate_branch("main").is_ok());
}

#[test]
fn accepts_commit_hash() {
    assert!(validate_branch("abc123def456").is_ok());
}

#[test]
fn rejects_empty_branch() {
    assert!(validate_branch("").is_err());
}

#[test]
fn rejects_branch_with_null() {
    assert!(validate_branch("main\0extra").is_err());
}

#[test]
fn rejects_branch_with_semicolon() {
    assert!(validate_branch("main; rm -rf /").is_err());
}

/// Regression: `Url::host_str` yields IPv6 literals wrapped in brackets, so
/// `[::1]` used to fail the `IpAddr` parse and slip through as a hostname.
#[test]
fn rejects_bracketed_ipv6_literals() {
    for url in [
        "https://[::1]/owner/repo",
        "https://[::ffff:127.0.0.1]/owner/repo",
        "https://[fc00::1]/owner/repo",
        "https://[fe80::1]/owner/repo",
        "https://[::]/owner/repo",
    ] {
        assert!(
            validate_repository_url(url).is_err(),
            "{url} should be rejected"
        );
    }
}

#[test]
fn rejects_reserved_ipv4_literals() {
    for url in [
        "https://127.0.0.1/owner/repo",
        "https://10.0.0.1/owner/repo",
        "https://169.254.169.254/owner/repo",
        "https://100.64.0.1/owner/repo",
        "https://0.0.0.0/owner/repo",
        "https://198.18.0.1/owner/repo",
        "https://240.0.0.1/owner/repo",
    ] {
        assert!(
            validate_repository_url(url).is_err(),
            "{url} should be rejected"
        );
    }
}

#[test]
fn still_accepts_ordinary_public_repository_urls() {
    for url in [
        "https://github.com/owner/repo",
        "https://github.com/owner/repo.git",
        "https://gitlab.com/owner/repo",
        "https://100.63.0.1/owner/repo",
    ] {
        assert!(validate_repository_url(url).is_ok(), "{url} should be ok");
    }
}

#[test]
fn is_blocked_ip_unwraps_ipv4_mapped_addresses() {
    let mapped: IpAddr = "::ffff:10.0.0.1".parse().unwrap();
    assert!(is_blocked_ip(mapped));
    let public: IpAddr = "::ffff:93.184.216.34".parse().unwrap();
    assert!(!is_blocked_ip(public));
}
