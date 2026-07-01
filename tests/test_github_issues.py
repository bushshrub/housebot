"""Tests for GitHubIssueReporter — configuration, JWT, token caching, and issue creation."""

import time
import pytest
from unittest.mock import AsyncMock, MagicMock, patch

from src.github_issues import GitHubIssueReporter


@pytest.fixture
def full_env(monkeypatch):
    monkeypatch.setenv("GITHUB_APP_ID", "12345")
    monkeypatch.setenv("GITHUB_APP_PRIVATE_KEY", "fake-key")
    monkeypatch.setenv("GITHUB_INSTALLATION_ID", "67890")
    monkeypatch.setenv("GITHUB_REPO", "owner/repo")


@pytest.fixture
def reporter(full_env):
    return GitHubIssueReporter()


@pytest.fixture
def unconfigured_reporter(monkeypatch):
    for var in (
        "GITHUB_APP_ID",
        "GITHUB_APP_PRIVATE_KEY",
        "GITHUB_INSTALLATION_ID",
        "GITHUB_REPO",
    ):
        monkeypatch.delenv(var, raising=False)
    return GitHubIssueReporter()


def _make_session_mock(responses):
    """Build an aiohttp.ClientSession mock that returns *responses* in order for .post()."""
    session = MagicMock()
    session.__aenter__ = AsyncMock(return_value=session)
    session.__aexit__ = AsyncMock(return_value=False)
    session.post = MagicMock(side_effect=responses)
    return session


def _make_response_mock(json_data, *, raise_for_status=None):
    resp = MagicMock()
    resp.json = AsyncMock(return_value=json_data)
    if raise_for_status is not None:
        resp.raise_for_status = MagicMock(side_effect=raise_for_status)
    else:
        resp.raise_for_status = MagicMock()
    resp.__aenter__ = AsyncMock(return_value=resp)
    resp.__aexit__ = AsyncMock(return_value=False)
    return resp


class TestIsConfigured:
    def test_all_vars_set(self, reporter):
        assert reporter.is_configured

    def test_missing_app_id(self, monkeypatch, full_env):
        monkeypatch.setenv("GITHUB_APP_ID", "")
        assert not GitHubIssueReporter().is_configured

    def test_missing_private_key(self, monkeypatch, full_env):
        monkeypatch.setenv("GITHUB_APP_PRIVATE_KEY", "")
        assert not GitHubIssueReporter().is_configured

    def test_missing_installation_id(self, monkeypatch, full_env):
        monkeypatch.setenv("GITHUB_INSTALLATION_ID", "")
        assert not GitHubIssueReporter().is_configured

    def test_missing_repo(self, monkeypatch, full_env):
        monkeypatch.setenv("GITHUB_REPO", "")
        assert not GitHubIssueReporter().is_configured

    def test_all_empty(self, unconfigured_reporter):
        assert not unconfigured_reporter.is_configured


class TestPrivateKeyNormalization:
    def test_escaped_newlines_replaced_with_literal(self, monkeypatch, full_env):
        monkeypatch.setenv("GITHUB_APP_PRIVATE_KEY", "header\\nbody\\nfooter")
        r = GitHubIssueReporter()
        assert r.private_key == "header\nbody\nfooter"

    def test_literal_newlines_preserved(self, monkeypatch, full_env):
        monkeypatch.setenv("GITHUB_APP_PRIVATE_KEY", "header\nbody")
        r = GitHubIssueReporter()
        assert r.private_key == "header\nbody"

    def test_empty_key_preserved(self, monkeypatch, full_env):
        monkeypatch.setenv("GITHUB_APP_PRIVATE_KEY", "")
        r = GitHubIssueReporter()
        assert r.private_key == ""


class TestInstallationTokenCaching:
    def test_cached_token_not_expired(self, reporter):
        reporter._installation_token = "tok"
        reporter._token_expires_at = time.time() + 3600
        assert reporter._installation_token == "tok"
        assert reporter._token_expires_at > time.time() + 60

    def test_expired_token_detected(self, reporter):
        reporter._installation_token = "old"
        reporter._token_expires_at = time.time() - 1
        assert reporter._token_expires_at < time.time()

    async def test_get_installation_token_fetches_when_expired(self, reporter):
        token_resp = _make_response_mock({"token": "ghs_fresh"})
        session = _make_session_mock([token_resp])

        # Bypass JWT signing — we only want to test the HTTP exchange here
        with patch.object(reporter, "_generate_jwt", return_value="fake.jwt.token"):
            with patch("aiohttp.ClientSession", return_value=session):
                token = await reporter._get_installation_token()

        assert token == "ghs_fresh"
        assert reporter._installation_token == "ghs_fresh"

    async def test_get_installation_token_reuses_cached(self, reporter):
        reporter._installation_token = "cached"
        reporter._token_expires_at = time.time() + 7200

        with patch("aiohttp.ClientSession") as mock_cs:
            token = await reporter._get_installation_token()

        assert token == "cached"
        mock_cs.assert_not_called()


class TestCreateIssue:
    async def test_returns_none_when_not_configured(self, unconfigured_reporter):
        result = await unconfigured_reporter.create_issue("title", "body")
        assert result is None

    async def test_returns_html_url_on_success(self, reporter):
        reporter._installation_token = "ghs_cached"
        reporter._token_expires_at = time.time() + 7200

        issue_resp = _make_response_mock(
            {"html_url": "https://github.com/owner/repo/issues/7"}
        )
        session = _make_session_mock([issue_resp])

        with patch("aiohttp.ClientSession", return_value=session):
            result = await reporter.create_issue(
                "My Issue", "Body text", labels=["bug"]
            )

        assert result == "https://github.com/owner/repo/issues/7"

    async def test_returns_none_on_http_error(self, reporter):
        reporter._installation_token = "ghs_cached"
        reporter._token_expires_at = time.time() + 7200

        err_resp = _make_response_mock({}, raise_for_status=Exception("HTTP 422"))
        session = _make_session_mock([err_resp])

        with patch("aiohttp.ClientSession", return_value=session):
            result = await reporter.create_issue("title", "body")

        assert result is None

    async def test_default_labels_is_bug(self, reporter):
        reporter._installation_token = "ghs_cached"
        reporter._token_expires_at = time.time() + 7200

        issue_resp = _make_response_mock(
            {"html_url": "https://github.com/owner/repo/issues/8"}
        )
        session = _make_session_mock([issue_resp])

        with patch("aiohttp.ClientSession", return_value=session):
            await reporter.create_issue("title", "body")

        call_kwargs = session.post.call_args.kwargs
        payload = call_kwargs.get("json") or {}
        assert payload.get("labels") == ["bug"]

    async def test_custom_labels_passed_through(self, reporter):
        reporter._installation_token = "ghs_cached"
        reporter._token_expires_at = time.time() + 7200

        issue_resp = _make_response_mock(
            {"html_url": "https://github.com/owner/repo/issues/9"}
        )
        session = _make_session_mock([issue_resp])

        with patch("aiohttp.ClientSession", return_value=session):
            await reporter.create_issue(
                "title", "body", labels=["enhancement", "help wanted"]
            )

        payload = session.post.call_args.kwargs.get("json") or {}
        assert payload.get("labels") == ["enhancement", "help wanted"]


class TestCreateErrorIssue:
    async def test_returns_none_when_not_configured(self, unconfigured_reporter):
        result = await unconfigured_reporter.create_error_issue("event-id-xyz")
        assert result is None

    async def test_title_references_sentry_event(self, reporter):
        captured = {}

        async def fake_create(title, body, labels=None):
            captured["title"] = title
            captured["body"] = body
            return "https://github.com/owner/repo/issues/10"

        with patch.object(reporter, "create_issue", side_effect=fake_create):
            await reporter.create_error_issue("abc-sentry-123")

        assert "abc-sentry-123" in captured["title"]
        assert "abc-sentry-123" in captured["body"]

    async def test_error_issue_gets_bug_label(self, reporter):
        captured = {}

        async def fake_create(title, body, labels=None):
            captured["labels"] = labels
            return "https://github.com/owner/repo/issues/11"

        with patch.object(reporter, "create_issue", side_effect=fake_create):
            await reporter.create_error_issue("some-event-id")

        assert captured.get("labels") == ["bug"]
