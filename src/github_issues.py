"""GitHub App integration for automatic error issue creation."""

import hashlib
import logging
import os
import time
import traceback
from datetime import datetime, timezone

import aiohttp
import jwt

logger = logging.getLogger(__name__)

# Don't re-file the same error within this window (seconds)
_DEDUP_WINDOW = 3600
_recent_errors: dict[str, float] = {}


def _error_fingerprint(exc: BaseException) -> str:
    tb = traceback.extract_tb(exc.__traceback__)
    # Use type + last few frames as the fingerprint
    key_frames = "".join(f"{f.filename}:{f.lineno}:{f.name}" for f in tb[-3:])
    raw = f"{type(exc).__name__}:{key_frames}"
    return hashlib.sha256(raw.encode()).hexdigest()[:16]


def _is_duplicate(fingerprint: str) -> bool:
    now = time.monotonic()
    # Prune stale entries
    expired = [k for k, t in _recent_errors.items() if now - t > _DEDUP_WINDOW]
    for k in expired:
        del _recent_errors[k]
    if fingerprint in _recent_errors:
        return True
    _recent_errors[fingerprint] = now
    return False


class GitHubIssueReporter:
    def __init__(self) -> None:
        self.app_id = os.getenv("GITHUB_APP_ID", "")
        raw_key = os.getenv("GITHUB_APP_PRIVATE_KEY", "")
        # Support both literal newlines and escaped \n (e.g. from .env files)
        self.private_key = raw_key.replace("\\n", "\n")
        self.installation_id = os.getenv("GITHUB_INSTALLATION_ID", "")
        self.repo = os.getenv("GITHUB_REPO", "")  # "owner/repo"
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
                "labels": labels or ["bug", "auto-reported"],
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

    async def report_error(
        self,
        exc: BaseException,
        context: dict[str, str] | None = None,
    ) -> str | None:
        """File an issue for exc unless a matching one was filed recently.

        Returns the issue URL if one was created, or None.
        """
        if not self.is_configured:
            return None

        fingerprint = _error_fingerprint(exc)
        if _is_duplicate(fingerprint):
            logger.debug("Skipping duplicate error report for fingerprint %s", fingerprint)
            return None

        ts = datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M:%S UTC")
        exc_type = type(exc).__name__
        exc_msg = str(exc)[:300]
        tb_text = "".join(traceback.format_exception(type(exc), exc, exc.__traceback__))

        title = f"[Auto] {exc_type}: {exc_msg[:80]}"

        ctx_section = ""
        if context:
            lines = "\n".join(f"- **{k}**: `{v}`" for k, v in context.items())
            ctx_section = f"\n## Context\n\n{lines}\n"

        body = f"""\
## Error Report

**Type:** `{exc_type}`
**Message:** {exc_msg}
**Fingerprint:** `{fingerprint}`
**Timestamp:** {ts}
{ctx_section}
## Traceback

```
{tb_text}
```

---
*Auto-reported by house-chatbot*"""

        return await self.create_issue(title, body, labels=["bug", "auto-reported"])
