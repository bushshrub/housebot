//! GitHub commit types and the deployment changelog renderer.

use crate::*;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct GitHubCommit {
    pub sha: String,
    pub html_url: String,
    pub commit: GitHubCommitDetails,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct GitHubCommitDetails {
    pub message: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct GitHubComparison {
    #[serde(default)]
    pub(crate) commits: Vec<GitHubCommit>,
}

pub fn commit_summary(selected: &GitHubCommit, recent: &[GitHubCommit]) -> String {
    let first_line = selected
        .commit
        .message
        .lines()
        .next()
        .unwrap_or("No commit message");
    let mut text = format!(
        "Deploying [`{}`]({}) — {}\n\n**Recent commits:**",
        short_sha(&selected.sha),
        selected.html_url,
        first_line
    );
    for commit in recent
        .iter()
        .filter(|commit| commit.sha != selected.sha)
        .take(3)
    {
        let message = commit
            .commit
            .message
            .lines()
            .next()
            .unwrap_or("No commit message");
        text.push_str(&format!(
            "\n• [`{}`]({}) — {}",
            short_sha(&commit.sha),
            commit.html_url,
            message
        ));
    }
    text
}

pub fn deployment_changelog(
    current_sha: &str,
    target_sha: &str,
    commits: &[GitHubCommit],
) -> String {
    if commits.is_empty() {
        return format!(
            "**Changelog**\n`{}` → `{}`\nNo commits found between these deployments.",
            short_sha(current_sha),
            short_sha(target_sha)
        );
    }

    let mut text = format!(
        "**Changelog since `{}`** ({} commit{})",
        short_sha(current_sha),
        commits.len(),
        if commits.len() == 1 { "" } else { "s" }
    );
    for (shown, commit) in commits.iter().enumerate() {
        let message = commit
            .commit
            .message
            .lines()
            .next()
            .unwrap_or("No commit message");
        let line = format!(
            "\n• [`{}`]({}) — {}",
            short_sha(&commit.sha),
            commit.html_url,
            message
        );
        if text.len() + line.len() > 1_800 {
            text.push_str(&format!(
                "\n• …and {} more commit(s)",
                commits.len() - shown
            ));
            break;
        }
        text.push_str(&line);
    }
    text
}
