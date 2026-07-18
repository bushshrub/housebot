//! `/github` slash command — native GitHub issue lifecycle management.

use crate::github_issues::GitHubIssueReporter;

pub(crate) async fn handle_github_interaction(
    reporter: &GitHubIssueReporter,
    options: &[serenity::all::CommandDataOption],
) -> String {
    let Some(action) = options.first() else {
        return "Usage: `/github list` | `/github show <number>` | `/github close <number>` | `/github search <query>`".into();
    };

    let action_options = nested_options(action).unwrap_or_default();

    match action.name.as_str() {
        "list" => {
            let state = string_option(action_options, "state").unwrap_or("open");
            let labels = string_option(action_options, "labels").unwrap_or("");
            reporter.list_issues(state, labels).await
        }
        "show" => {
            let issue_number = string_option(action_options, "number")
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            if issue_number == 0 {
                return "Error: a valid issue number is required for `/github show`.".into();
            }
            match reporter.get_issue_detail(issue_number).await {
                Some(detail) => detail,
                None => "Error: failed to fetch issue detail. Is GitHub configured?".into(),
            }
        }
        "close" => {
            let issue_number = string_option(action_options, "number")
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            if issue_number == 0 {
                return "Error: a valid issue number is required for `/github close`.".into();
            }
            if reporter.close_issue(issue_number).await {
                format!("✅ Issue **#{issue_number}** closed successfully.")
            } else {
                format!("⚠️ Failed to close issue #{issue_number}. Is GitHub configured?")
            }
        }
        "search" => {
            let query = string_option(action_options, "query").unwrap_or("");
            if query.is_empty() {
                return "Error: a search query is required for `/github search`.".into();
            }
            reporter.search_issues(query).await
        }
        other => format!("Unknown subcommand `{other}`. Use `/github list|show|close|search`."),
    }
}

use super::{nested_options, string_option};
