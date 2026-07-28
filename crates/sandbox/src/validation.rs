use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::limits;

/// Validate a public HTTPS repository URL for cloning.
///
/// Accepts:
///   - `https://github.com/owner/repo` (with optional `.git` suffix and extra path segments)
///   - `https://gitlab.com/owner/repo`
///   - Any `https://` URL that does not point to a private or local address.
///
/// Rejects:
///   - Non-HTTPS schemes (`http`, `ssh`, `git`, `file`, `ftp`)
///   - Embedded credentials (`user:pass@host`)
///   - `file://` URLs
///   - Localhost and private-network destinations
///   - URLs exceeding length limits
pub fn validate_repository_url(url: &str) -> Result<(), String> {
    if url.len() > limits::MAX_URL_LENGTH {
        return Err(format!(
            "URL exceeds maximum length of {} characters",
            limits::MAX_URL_LENGTH
        ));
    }

    let parsed = url::Url::parse(url).map_err(|e| format!("Invalid URL: {e}"))?;

    let scheme = parsed.scheme();
    if scheme != "https" {
        return Err(format!(
            "Only HTTPS URLs are supported (got scheme '{scheme}')"
        ));
    }

    // Reject embedded credentials
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("URL must not contain embedded credentials".to_string());
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| "URL has no host".to_string())?;

    // Reject localhost and private IPs
    if is_private_or_local(host) {
        return Err(format!("URL points to a private or local address: {host}"));
    }

    Ok(())
}

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

/// Whether `ip` is anything other than a routable public address.
///
/// IPv4-mapped and IPv4-compatible IPv6 addresses are unwrapped first —
/// `::ffff:127.0.0.1` is loopback no matter which family it is written in, and
/// `Ipv6Addr::is_loopback` does not see through the mapping.
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_blocked_v4(ip),
        IpAddr::V6(ip) => match ip.to_ipv4() {
            Some(mapped) => is_blocked_v4(mapped),
            None => is_blocked_v6(ip),
        },
    }
}

fn is_blocked_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_multicast()
        || ip.is_broadcast()
        // 0.0.0.0/8 "this network"
        || a == 0
        // 100.64.0.0/10 carrier-grade NAT
        || (a == 100 && (64..128).contains(&b))
        // 192.0.0.0/24 IETF protocol assignments
        || (a == 192 && b == 0 && c == 0)
        // 198.18.0.0/15 benchmarking
        || (a == 198 && (b == 18 || b == 19))
        // 240.0.0.0/4 reserved
        || a >= 240
}

fn is_blocked_v6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        // fc00::/7 unique local
        || (segments[0] & 0xfe00) == 0xfc00
        // fe80::/10 link local
        || (segments[0] & 0xffc0) == 0xfe80
        // 100::/64 discard-only
        || segments[0..4] == [0x0100, 0, 0, 0]
        // 2001:db8::/32 documentation
        || (segments[0] == 0x2001 && segments[1] == 0x0db8)
}

/// Check whether a URL host names a private or local destination.
///
/// `Url::host_str` returns IPv6 literals in their bracketed form, which does not
/// parse as an `IpAddr` — the brackets are stripped here so address literals are
/// actually classified rather than falling through to the hostname checks.
fn is_private_or_local(host: &str) -> bool {
    let literal = host.strip_prefix('[').and_then(|h| h.strip_suffix(']'));
    if let Ok(ip) = literal.unwrap_or(host).parse::<IpAddr>() {
        return is_blocked_ip(ip);
    }

    host.eq_ignore_ascii_case("localhost")
        || host.eq_ignore_ascii_case("local")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host.ends_with(".lan")
        || host.ends_with(".localdomain")
        || host.ends_with(".localhost")
        || host == "host.docker.internal"
}

/// Validate a search query string.
pub fn validate_query(query: &str) -> Result<(), String> {
    if query.is_empty() {
        return Err("Search query must not be empty".to_string());
    }
    if query.len() > limits::MAX_QUERY_LENGTH {
        return Err(format!(
            "Search query exceeds maximum length of {} characters",
            limits::MAX_QUERY_LENGTH
        ));
    }
    if query.contains('\0') {
        return Err("Search query contains null byte".to_string());
    }
    Ok(())
}

/// Validate a glob pattern string.
pub fn validate_glob(glob: &str) -> Result<(), String> {
    if glob.len() > limits::MAX_GLOB_LENGTH {
        return Err("Glob pattern exceeds maximum length".to_string());
    }
    if glob.contains('\0') {
        return Err("Glob pattern contains null byte".to_string());
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

/// Validate a branch or commit reference.
pub fn validate_branch(branch: &str) -> Result<(), String> {
    if branch.is_empty() {
        return Err("Branch must not be empty".to_string());
    }
    if branch.len() > limits::MAX_CLONE_BRANCH_LENGTH {
        return Err(format!(
            "Branch/commit reference exceeds maximum length of {} characters",
            limits::MAX_CLONE_BRANCH_LENGTH
        ));
    }
    // Block potentially dangerous patterns
    if branch.contains('\0')
        || branch.contains('\n')
        || branch.contains(';')
        || branch.contains('&')
    {
        return Err("Branch contains invalid characters".to_string());
    }
    Ok(())
}

#[cfg(test)]
#[path = "validation_tests.rs"]
mod tests;
