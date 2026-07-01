"""Tests for the opencode sandbox tool — workspace management and artifact collection."""

import os
import pytest
from unittest.mock import MagicMock, patch

import src.tools.opencode as oc_mod
from src.tools.opencode import _collect_workspace_files, _make_workspace


@pytest.fixture(autouse=True)
def patch_dirs(tmp_path, monkeypatch):
    artifacts = tmp_path / "artifacts"
    artifacts.mkdir()
    workspaces = tmp_path / "workspaces"
    workspaces.mkdir()
    monkeypatch.setattr(oc_mod, "ARTIFACTS_DIR", str(artifacts))
    monkeypatch.setattr(oc_mod, "CONTAINER_DATA_DIR", str(tmp_path))
    monkeypatch.setattr(oc_mod, "HOST_DATA_DIR", "")
    return tmp_path


class TestMakeWorkspace:
    def test_creates_directory(self):
        _, container = _make_workspace()
        assert os.path.isdir(container)

    def test_host_equals_container_when_no_host_data_dir(self):
        host, container = _make_workspace()
        assert host == container

    def test_host_differs_when_host_data_dir_set(self, monkeypatch):
        monkeypatch.setattr(oc_mod, "HOST_DATA_DIR", "/host/data")
        host, container = _make_workspace()
        assert host.startswith("/host/data")
        assert host != container

    def test_each_call_returns_unique_path(self):
        _, c1 = _make_workspace()
        _, c2 = _make_workspace()
        assert c1 != c2

    def test_directory_is_world_writable(self):
        _, container = _make_workspace()
        # Permissions should include all write bits (0o777)
        mode = os.stat(container).st_mode & 0o777
        assert mode == 0o777

    def test_workspace_is_inside_workspaces_subdir(self):
        _, container = _make_workspace()
        assert "workspaces" in container


class TestCollectWorkspaceFiles:
    def _workspace(self, tmp_path, name="ws"):
        d = tmp_path / name
        d.mkdir(exist_ok=True)
        return d

    def test_collects_regular_files(self, tmp_path):
        ws = self._workspace(tmp_path)
        (ws / "hello.txt").write_text("content")
        collected = _collect_workspace_files(str(ws))
        assert len(collected) == 1

    def test_skips_opencode_json(self, tmp_path):
        ws = self._workspace(tmp_path)
        (ws / "opencode.json").write_text("{}")
        (ws / "other.py").write_text("code")
        collected = _collect_workspace_files(str(ws))
        assert len(collected) == 1
        assert all("opencode.json" not in p for p in collected)

    def test_skips_dot_opencode_json(self, tmp_path):
        ws = self._workspace(tmp_path)
        (ws / ".opencode.json").write_text("{}")
        collected = _collect_workspace_files(str(ws))
        assert collected == []

    def test_skips_dotfiles_in_root(self, tmp_path):
        ws = self._workspace(tmp_path)
        (ws / ".hidden").write_text("secret")
        (ws / "visible.py").write_text("code")
        collected = _collect_workspace_files(str(ws))
        assert len(collected) == 1
        assert all(".hidden" not in p for p in collected)

    def test_skips_dotdirectories_recursively(self, tmp_path):
        ws = self._workspace(tmp_path)
        dot_dir = ws / ".git"
        dot_dir.mkdir()
        (dot_dir / "config").write_text("[core]")
        (ws / "main.py").write_text("hello")
        collected = _collect_workspace_files(str(ws))
        assert len(collected) == 1

    def test_skips_oversized_files(self, tmp_path, monkeypatch):
        monkeypatch.setattr(oc_mod, "MAX_ARTIFACT_SIZE_MB", 0)
        ws = self._workspace(tmp_path)
        (ws / "big.bin").write_bytes(b"x" * 10)
        collected = _collect_workspace_files(str(ws))
        assert collected == []

    def test_within_size_limit_collected(self, tmp_path, monkeypatch):
        monkeypatch.setattr(oc_mod, "MAX_ARTIFACT_SIZE_MB", 1)
        ws = self._workspace(tmp_path)
        (ws / "small.txt").write_bytes(b"x" * 100)
        collected = _collect_workspace_files(str(ws))
        assert len(collected) == 1

    def test_nested_files_collected(self, tmp_path):
        ws = self._workspace(tmp_path)
        sub = ws / "subdir"
        sub.mkdir()
        (sub / "nested.py").write_text("code")
        collected = _collect_workspace_files(str(ws))
        assert len(collected) == 1

    def test_nested_files_get_flat_names_with_separator(self, tmp_path):
        ws = self._workspace(tmp_path)
        sub = ws / "subdir"
        sub.mkdir()
        (sub / "nested.py").write_text("code")
        collected = _collect_workspace_files(str(ws))
        assert "subdir_nested.py" in collected[0]

    def test_empty_workspace_returns_empty_list(self, tmp_path):
        ws = self._workspace(tmp_path)
        assert _collect_workspace_files(str(ws)) == []

    def test_multiple_files_all_collected(self, tmp_path):
        ws = self._workspace(tmp_path)
        for i in range(5):
            (ws / f"file{i}.txt").write_text(f"content {i}")
        collected = _collect_workspace_files(str(ws))
        assert len(collected) == 5

    def test_artifacts_written_to_artifacts_dir(self, tmp_path):
        ws = self._workspace(tmp_path)
        (ws / "output.txt").write_text("result")
        collected = _collect_workspace_files(str(ws))
        artifacts_dir = str(oc_mod.ARTIFACTS_DIR)
        assert all(p.startswith(artifacts_dir) for p in collected)

    def test_artifact_files_are_copies(self, tmp_path):
        ws = self._workspace(tmp_path)
        source = ws / "data.txt"
        source.write_text("my data")
        collected = _collect_workspace_files(str(ws))
        assert len(collected) == 1
        assert open(collected[0]).read() == "my data"
        assert collected[0] != str(source)

    def test_uid_prefix_avoids_name_collisions(self, tmp_path):
        ws1 = self._workspace(tmp_path, "ws1")
        ws2 = self._workspace(tmp_path, "ws2")
        (ws1 / "file.txt").write_text("a")
        (ws2 / "file.txt").write_text("b")
        c1 = _collect_workspace_files(str(ws1))
        c2 = _collect_workspace_files(str(ws2))
        assert c1[0] != c2[0]


class TestRunOpencodeDockerError:
    """Unit-level tests for the Docker error-path branches."""

    async def test_returns_error_string_when_docker_unavailable(self):
        import docker.errors

        with patch(
            "docker.from_env",
            side_effect=docker.errors.DockerException("socket not found"),
        ):
            result = await oc_mod.run_opencode("do something")
        assert isinstance(result, str)
        assert "error" in result.lower()

    async def test_returns_error_string_when_image_not_found(self, monkeypatch):
        import docker.errors

        mock_client = MagicMock()
        mock_client.containers.run.side_effect = docker.errors.ImageNotFound(
            "not found"
        )
        with patch("docker.from_env", return_value=mock_client):
            result = await oc_mod.run_opencode("do something")
        assert isinstance(result, str)
        assert "error" in result.lower()
