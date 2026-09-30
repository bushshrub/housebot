use crate::limits;

/// Validate a workspace-relative path.
///
/// Accepts normal relative paths within the workspace.
/// Rejects:
///   - Absolute paths
///   - `..` traversal
///   - Null bytes
///   - Symlink escape (checked at the server level)
pub fn validate_workspace_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("Path must not be empty".to_string());
    }
    if path.len() > 512 {
        return Err("Path exceeds maximum length".to_string());
    }
    if path.contains('\0') {
        return Err("Path contains null byte".to_string());
    }
    if std::path::Path::new(path).is_absolute() {
        return Err("Absolute paths are not allowed; use a workspace-relative path".to_string());
    }
    // Check for directory traversal
    let mut depth = 0i32;
    for component in std::path::Path::new(path).components() {
        match component {
            std::path::Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return Err("Path escapes the workspace via '..'".to_string());
                }
            }
            std::path::Component::Normal(_) => {
                depth += 1;
                if depth as usize > limits::MAX_PATH_DEPTH {
                    return Err("Path exceeds maximum depth".to_string());
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Validate a command string.
pub fn validate_command(command: &str) -> Result<(), String> {
    if command.is_empty() {
        return Err("Command must not be empty".to_string());
    }
    if command.len() > limits::MAX_COMMAND_LENGTH {
        return Err(format!(
            "Command exceeds maximum length of {} characters",
            limits::MAX_COMMAND_LENGTH
        ));
    }
    if command.contains('\0') {
        return Err("Command contains null byte".to_string());
    }
    Ok(())
}

/// Session keys are opaque identifiers: sandboxd matches on them and logs them,
/// but never passes them to Docker. Bounded and restricted anyway so a
/// malformed key cannot bloat the map or smuggle control characters into logs.
pub fn validate_session_key(session_key: &str) -> Result<(), String> {
    if session_key.is_empty() {
        return Err("Session key must not be empty".to_string());
    }
    if session_key.len() > limits::MAX_SESSION_KEY_LENGTH {
        return Err(format!(
            "Session key exceeds maximum length of {} characters",
            limits::MAX_SESSION_KEY_LENGTH
        ));
    }
    if !session_key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("Session key contains invalid characters".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Session key validation ────────────────────────────────────────────

    #[test]
    fn accepts_a_discord_user_id_as_a_session_key() {
        assert!(validate_session_key("123456789012345678").is_ok());
    }

    #[test]
    fn rejects_empty_session_key() {
        assert!(validate_session_key("").is_err());
    }

    #[test]
    fn rejects_oversized_session_key() {
        assert!(validate_session_key(&"a".repeat(limits::MAX_SESSION_KEY_LENGTH + 1)).is_err());
    }

    #[test]
    fn rejects_session_key_with_control_characters() {
        for key in ["user 1", "user/1", "user;rm", "user\n1", "user\0"] {
            assert!(
                validate_session_key(key).is_err(),
                "session keys are plain identifiers: {key}"
            );
        }
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
}
