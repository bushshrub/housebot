"""Tests for pure helpers in src/tools/opencode.py."""

import os
import pytest

from src.tools.opencode import _collect_workspace_files, _EXCLUDED_FILENAMES


@pytest.fixture()
def workspace(tmp_path):
    """Return a temporary workspace directory path."""
    ws = tmp_path / "workspace"
    ws.mkdir()
    return ws


class TestCollectWorkspaceFiles:
    def test_empty_workspace_returns_empty(self, workspace, tmp_path, monkeypatch):
        monkeypatch.setattr("src.tools.opencode.ARTIFACTS_DIR", str(tmp_path / "artifacts"))
        result = _collect_workspace_files(str(workspace))
        assert result == []

    def test_regular_file_is_collected(self, workspace, tmp_path, monkeypatch):
        monkeypatch.setattr("src.tools.opencode.ARTIFACTS_DIR", str(tmp_path / "artifacts"))
        (workspace / "hello.py").write_text("print('hi')")
        result = _collect_workspace_files(str(workspace))
        assert len(result) == 1
        assert result[0].endswith(".py") or "hello" in result[0]

    def test_excluded_filename_opencode_json_is_skipped(self, workspace, tmp_path, monkeypatch):
        monkeypatch.setattr("src.tools.opencode.ARTIFACTS_DIR", str(tmp_path / "artifacts"))
        (workspace / "opencode.json").write_text("{}")
        result = _collect_workspace_files(str(workspace))
        assert result == []

    def test_excluded_filename_dot_opencode_json_is_skipped(self, workspace, tmp_path, monkeypatch):
        monkeypatch.setattr("src.tools.opencode.ARTIFACTS_DIR", str(tmp_path / "artifacts"))
        (workspace / ".opencode.json").write_text("{}")
        result = _collect_workspace_files(str(workspace))
        assert result == []

    def test_dotfile_is_skipped(self, workspace, tmp_path, monkeypatch):
        monkeypatch.setattr("src.tools.opencode.ARTIFACTS_DIR", str(tmp_path / "artifacts"))
        (workspace / ".gitignore").write_text("*.pyc")
        result = _collect_workspace_files(str(workspace))
        assert result == []

    def test_dotdir_contents_are_skipped(self, workspace, tmp_path, monkeypatch):
        monkeypatch.setattr("src.tools.opencode.ARTIFACTS_DIR", str(tmp_path / "artifacts"))
        hidden = workspace / ".git"
        hidden.mkdir()
        (hidden / "config").write_text("data")
        result = _collect_workspace_files(str(workspace))
        assert result == []

    def test_oversized_file_is_skipped(self, workspace, tmp_path, monkeypatch):
        monkeypatch.setattr("src.tools.opencode.ARTIFACTS_DIR", str(tmp_path / "artifacts"))
        monkeypatch.setattr("src.tools.opencode.MAX_ARTIFACT_SIZE_MB", 0)
        (workspace / "big.txt").write_text("data")
        result = _collect_workspace_files(str(workspace))
        assert result == []

    def test_multiple_files_all_collected(self, workspace, tmp_path, monkeypatch):
        monkeypatch.setattr("src.tools.opencode.ARTIFACTS_DIR", str(tmp_path / "artifacts"))
        (workspace / "a.py").write_text("a")
        (workspace / "b.sh").write_text("b")
        result = _collect_workspace_files(str(workspace))
        assert len(result) == 2

    def test_nested_file_is_collected(self, workspace, tmp_path, monkeypatch):
        monkeypatch.setattr("src.tools.opencode.ARTIFACTS_DIR", str(tmp_path / "artifacts"))
        sub = workspace / "src"
        sub.mkdir()
        (sub / "main.py").write_text("code")
        result = _collect_workspace_files(str(workspace))
        assert len(result) == 1

    def test_collected_files_exist_in_artifacts_dir(self, workspace, tmp_path, monkeypatch):
        artifacts = tmp_path / "artifacts"
        monkeypatch.setattr("src.tools.opencode.ARTIFACTS_DIR", str(artifacts))
        (workspace / "out.txt").write_text("output")
        result = _collect_workspace_files(str(workspace))
        assert len(result) == 1
        assert os.path.isfile(result[0])

    def test_excluded_filenames_constant_contains_expected(self):
        assert "opencode.json" in _EXCLUDED_FILENAMES
        assert ".opencode.json" in _EXCLUDED_FILENAMES
