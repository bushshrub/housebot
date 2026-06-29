"""GitHub App integration for creating issues. Errors go to Sentry; issues reference the event ID."""

import logging
import os
import time

import aiohttp
import jwt

logger = logging.getLogger(__name__)


class GitHubIssueReporter:
    def __init__(self) -> None:
        self.app_id = os.getenv("GITHUB_APP_ID", "")
        raw_key = os.getenv("GITHUB_APP_PRIVATE_KEY", "")
        self.private_key = raw_key.replace("\\n", "\n")
        self.installation_id = os.getenv("GITHUB_INSTALLATION_ID", "")
        self.repo = os.getenv("GITHUB_REPO", "")
        self._installation_token: str | None = None
        self._token_expires_at: float = 0

    @property
    def is_configured(self) -> bool:
        return bool(self.app_id and self.private_key and self.installation_id and self.repo)

    def _generate_jwt(self) -> str:
        now = int(time.time())
        payload = {
            "iat": now - 60,
            "exp": now + 600,
            "iss": self.app_id,
        }
        return jwt.encode(payload, self.private_key, algorithm="RS256")

    async def _get_installation_token(self) -> str:
        if self._installation_token and time.time() < self._token_expires_at - 60:
            return self._installation_token

        jwt_token = self._generate_jwt()
        url = f"https://api.github.com/app/installations/{self.installation_id}/access_tokens"
        headers = {
            "Authorization": f"Bearer {jwt_token}",
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
        }
        async with aiohttp.ClientSession() as session:
            async with session.post(url, headers=headers) as resp:
                resp.raise_for_status()
                data = await resp.json()

        self._installation_token = data["token"]
        self._token_expires_at = time.time() + 3600
        return self._installation_token

    async def create_issue(
        self,
        title: str,
        body: str,
        labels: list[str] | None = None,
    ) -> str | None:
        """Create a GitHub issue and return its URL, or None on failure."""
        if not self.is_configured:
            return None

        try:
            token = await self._get_installation_token()
            url = f"https://api.github.com/repos/{self.repo}/issues"
            headers = {
                "Authorization": f"Bearer {token}",
                "Accept": "application/vnd.github+json",
                "X-GitHub-Api-Version": "2022-11-28",
            }
            payload = {
                "title": title,
                "body": body,
                "labels": labels or ["bug"],
            }
            async with aiohttp.ClientSession() as session:
                async with session.post(url, headers=headers, json=payload) as resp:
                    resp.raise_for_status()
                    data = await resp.json()
            issue_url: str = data["html_url"]
            logger.info("Created GitHub issue: %s", issue_url)
            return issue_url
        except Exception:
            logger.exception("Failed to create GitHub issue")
            return None

    async def create_error_issue(self, sentry_event_id: str) -> str | None:
        """Create a GitHub issue referencing a Sentry event. No sensitive data in the body."""
        if not self.is_configured:
            return None

        title = f"Bot error — Sentry event {sentry_event_id}"
        body = (
            "An error occurred in the bot. Details are available in Sentry.\n\n"
            f"Sentry Event ID: `{sentry_event_id}`\n"
        )
        return await self.create_issue(title, body, labels=["bug"])
