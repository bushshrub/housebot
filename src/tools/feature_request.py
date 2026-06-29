"""Tool for creating GitHub feature request issues from user requests."""

import logging
from datetime import datetime, timezone

from ..github_issues import GitHubIssueReporter

logger = logging.getLogger(__name__)

TOOL_DEFINITION = {
    "name": "create_feature_request",
    "description": (
        "Create a GitHub issue to track a feature request made by a user. "
        "Use this whenever a user asks for a new feature or improvement to the bot. "
        "Returns the URL of the created issue."
    ),
    "input_schema": {
        "type": "object",
        "properties": {
            "title": {
                "type": "string",
                "description": "Short, clear title for the feature request (under 100 chars).",
            },
            "description": {
                "type": "string",
                "description": "Full description of the requested feature, including context and motivation.",
            },
        },
        "required": ["title", "description"],
    },
}

_reporter: GitHubIssueReporter | None = None


def _get_reporter() -> GitHubIssueReporter:
    global _reporter
    if _reporter is None:
        _reporter = GitHubIssueReporter()
    return _reporter


async def create_feature_request(
    title: str,
    description: str,
    requested_by: str = "unknown",
) -> str:
    reporter = _get_reporter()
    if not reporter.is_configured:
        return "Error: GitHub integration is not configured — feature request was not filed."

    ts = datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M:%S UTC")
    body = f"""\
## Feature Request

**Requested by:** {requested_by}
**Timestamp:** {ts}

## Description

{description}

---
*Filed automatically by house-chatbot*"""

    url = await reporter.create_issue(title=title, body=body, labels=["enhancement"])
    if url is None:
        return "Error: Failed to create GitHub issue — check bot logs for details."
    return f"Feature request filed: {url}"
