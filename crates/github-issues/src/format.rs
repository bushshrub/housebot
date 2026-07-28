//! Response formatting helpers shared by the query endpoints.

/// Format a GitHub issues API response as a compact text list.
pub(crate) fn format_issue_list(body: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(val) => {
            let issues: Vec<&serde_json::Value> = if let Some(arr) = val.as_array() {
                arr.iter()
                    .filter(|i| i.get("pull_request").is_none())
                    .collect()
            } else if let Some(items) = val.get("items").and_then(|v| v.as_array()) {
                items
                    .iter()
                    .filter(|i| i.get("pull_request").is_none())
                    .collect()
            } else {
                return "Error: unexpected API response format.".to_string();
            };
            if issues.is_empty() {
                return "No issues found.".to_string();
            }
            let lines: Vec<String> = issues
                .iter()
                .map(|i| {
                    let number = i["number"].as_u64().unwrap_or(0);
                    let title = i["title"].as_str().unwrap_or("(untitled)");
                    let state = i["state"].as_str().unwrap_or("unknown");
                    let labels: Vec<String> = i["labels"]
                        .as_array()
                        .map(|labels| {
                            labels
                                .iter()
                                .filter_map(|l| l["name"].as_str().map(|n| n.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    let label_str = if labels.is_empty() {
                        String::new()
                    } else {
                        format!(" [{}]", labels.join(", "))
                    };
                    format!("#{number} ({state}){label_str} — {title}")
                })
                .collect();
            lines.join("\n")
        }
        Err(e) => format!("Error: failed to parse response — {e}"),
    }
}

/// Percent-encode a string for URL query parameters.
pub(crate) fn urlencoding(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            b' ' => result.push_str("%20"),
            _ => {
                result.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    result
}

/// Extract the `rel="next"` page URL from a GitHub API response's Link header.
pub(crate) fn next_page_url(resp: &reqwest::Response) -> Option<String> {
    let link = resp.headers().get("link")?.to_str().ok()?;
    for part in link.split('<').skip(1) {
        let end = part.find('>')?;
        let url = &part[..end];
        if part[end..].contains("rel=\"next\"") {
            return Some(url.to_string());
        }
    }
    None
}
