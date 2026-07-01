"""Tests for the create_feature_request tool — rate limiting and integration."""

import time
import pytest
from unittest.mock import AsyncMock, MagicMock, patch

import src.tools.feature_request as fr_mod
from src.tools.feature_request import (
    RATE_LIMIT_MAX_REQUESTS,
    RATE_LIMIT_WINDOW_SECONDS,
    _is_rate_limited,
    create_feature_request,
)


@pytest.fixture(autouse=True)
def reset_rate_limit_state():
    fr_mod._request_timestamps.clear()
    yield
    fr_mod._request_timestamps.clear()


class TestIsRateLimited:
    def test_first_request_allowed(self):
        assert not _is_rate_limited("user1")

    def test_under_limit_allowed(self):
        for _ in range(RATE_LIMIT_MAX_REQUESTS - 1):
            assert not _is_rate_limited("user2")

    def test_at_limit_blocked(self):
        for _ in range(RATE_LIMIT_MAX_REQUESTS):
            _is_rate_limited("user3")
        assert _is_rate_limited("user3")

    def test_exceeding_limit_stays_blocked(self):
        for _ in range(RATE_LIMIT_MAX_REQUESTS + 5):
            _is_rate_limited("user_x")
        assert _is_rate_limited("user_x")

    def test_different_users_are_independent(self):
        for _ in range(RATE_LIMIT_MAX_REQUESTS):
            _is_rate_limited("userA")
        assert _is_rate_limited("userA")
        assert not _is_rate_limited("userB")

    def test_expired_timestamps_are_evicted(self):
        now = time.monotonic()
        fr_mod._request_timestamps["user4"] = [
            now - RATE_LIMIT_WINDOW_SECONDS - 10
        ] * RATE_LIMIT_MAX_REQUESTS
        assert not _is_rate_limited("user4")

    def test_mixed_old_and_fresh_timestamps(self):
        now = time.monotonic()
        # One expired, (MAX-1) fresh → after eviction only (MAX-1) remain → one more allowed
        fr_mod._request_timestamps["user5"] = [now - RATE_LIMIT_WINDOW_SECONDS - 1] + [
            now - 1
        ] * (RATE_LIMIT_MAX_REQUESTS - 1)
        assert not _is_rate_limited("user5")
        assert _is_rate_limited("user5")

    def test_rate_limit_consumes_slot(self):
        initial_len = len(fr_mod._request_timestamps.get("user6", []))
        _is_rate_limited("user6")
        assert len(fr_mod._request_timestamps["user6"]) == initial_len + 1

    def test_rate_limit_does_not_consume_slot_when_blocked(self):
        for _ in range(RATE_LIMIT_MAX_REQUESTS):
            _is_rate_limited("user7")
        count_before = len(fr_mod._request_timestamps["user7"])
        _is_rate_limited("user7")
        assert len(fr_mod._request_timestamps["user7"]) == count_before


class TestCreateFeatureRequest:
    def _mock_reporter(
        self, *, is_configured=True, issue_url="https://github.com/o/r/issues/1"
    ):
        reporter = MagicMock()
        reporter.is_configured = is_configured
        reporter.create_issue = AsyncMock(return_value=issue_url)
        return reporter

    async def test_unconfigured_reporter_returns_error(self):
        reporter = self._mock_reporter(is_configured=False)
        with patch("src.tools.feature_request._get_reporter", return_value=reporter):
            result = await create_feature_request("title", "desc", requested_by="u1")
        assert "not configured" in result.lower()

    async def test_rate_limited_returns_error_message(self):
        reporter = self._mock_reporter()
        with patch("src.tools.feature_request._get_reporter", return_value=reporter):
            for _ in range(RATE_LIMIT_MAX_REQUESTS):
                await create_feature_request("t", "d", requested_by="u2")
            result = await create_feature_request("t", "d", requested_by="u2")
        assert "rate limit" in result.lower()

    async def test_successful_request_returns_url(self):
        reporter = self._mock_reporter(
            issue_url="https://github.com/owner/repo/issues/42"
        )
        with patch("src.tools.feature_request._get_reporter", return_value=reporter):
            result = await create_feature_request(
                "Add dark mode", "...", requested_by="u3"
            )
        assert "https://github.com" in result
        assert "42" in result

    async def test_failed_issue_creation_returns_error(self):
        reporter = self._mock_reporter(issue_url=None)
        with patch("src.tools.feature_request._get_reporter", return_value=reporter):
            result = await create_feature_request("t", "d", requested_by="u4")
        assert "error" in result.lower() or "failed" in result.lower()

    async def test_issue_body_contains_requester(self):
        reporter = self._mock_reporter()
        with patch("src.tools.feature_request._get_reporter", return_value=reporter):
            await create_feature_request(
                "My feature", "Details here", requested_by="alice"
            )
        call_kwargs = reporter.create_issue.call_args.kwargs
        body = call_kwargs.get("body") or reporter.create_issue.call_args.args[1]
        assert "alice" in body

    async def test_issue_body_contains_description(self):
        reporter = self._mock_reporter()
        with patch("src.tools.feature_request._get_reporter", return_value=reporter):
            await create_feature_request(
                "Title", "Detailed description here", requested_by="bob"
            )
        call_kwargs = reporter.create_issue.call_args.kwargs
        body = call_kwargs.get("body") or reporter.create_issue.call_args.args[1]
        assert "Detailed description here" in body

    async def test_issue_gets_enhancement_label(self):
        reporter = self._mock_reporter()
        with patch("src.tools.feature_request._get_reporter", return_value=reporter):
            await create_feature_request("t", "d", requested_by="u5")
        call_kwargs = reporter.create_issue.call_args.kwargs
        labels = call_kwargs.get("labels") or (
            reporter.create_issue.call_args.args[2]
            if len(reporter.create_issue.call_args.args) > 2
            else None
        )
        assert labels == ["enhancement"]

    async def test_issue_title_matches_input(self):
        reporter = self._mock_reporter()
        with patch("src.tools.feature_request._get_reporter", return_value=reporter):
            await create_feature_request("Exact Title Here", "d", requested_by="u6")
        call_kwargs = reporter.create_issue.call_args.kwargs
        title = call_kwargs.get("title") or reporter.create_issue.call_args.args[0]
        assert title == "Exact Title Here"

    async def test_default_requester_does_not_crash(self):
        reporter = self._mock_reporter()
        with patch("src.tools.feature_request._get_reporter", return_value=reporter):
            result = await create_feature_request("t", "d")
        assert result  # just verifies no exception
