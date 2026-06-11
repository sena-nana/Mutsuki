from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import pytest


def _repo_root() -> Path:
    return Path(__file__).resolve().parents[3]


def _read_json(path: Path) -> dict[str, Any]:
    loaded = json.loads(path.read_text(encoding="utf-8"))
    assert isinstance(loaded, dict)
    return loaded


def test_local_plugin_marketplace_entries_are_installable() -> None:
    repo_root = _repo_root()
    plugin_root = repo_root / ".agents" / "plugins"
    marketplace = _read_json(plugin_root / "marketplace.json")

    plugins = marketplace["plugins"]
    assert isinstance(plugins, list)
    names = {item["name"] for item in plugins}
    assert {
        "mutsukicore-codex-core",
        "mutsukicore-claude-core",
        "mutsukicore-test-io",
    } <= names

    for item in plugins:
        assert isinstance(item, dict)
        source = item["source"]
        assert isinstance(source, dict)
        assert source["source"] == "local"
        plugin_path = plugin_root / str(source["path"])
        plugin_path = plugin_path.resolve()
        assert plugin_path.is_dir()

        manifest = _read_json(plugin_path / ".codex-plugin" / "plugin.json")
        assert manifest["name"] == item["name"]
        assert isinstance(manifest["version"], str)
        assert isinstance(manifest["description"], str)
        assert isinstance(manifest["interface"], dict)
        assert manifest["interface"]["displayName"]

        skills_path = plugin_path / str(manifest["skills"])
        assert skills_path.is_dir()
        assert any(skills_path.glob("*/SKILL.md"))


def test_test_io_mcp_config_points_to_existing_server_script() -> None:
    plugin_path = _repo_root() / ".agents" / "plugins" / "plugins" / "mutsukicore-test-io"
    manifest = _read_json(plugin_path / ".codex-plugin" / "plugin.json")
    mcp_path = plugin_path / str(manifest["mcpServers"])
    mcp = _read_json(mcp_path)

    server = mcp["mcpServers"]["mutsukicore-test-io"]
    assert server["command"] == "python"
    args = server["args"]
    assert isinstance(args, list)
    assert args == ["./scripts/mutsukicore_test_io_mcp.py"]
    assert (plugin_path / args[0]).is_file()


@pytest.mark.parametrize(
    ("plugin_name", "display_name", "skill_dir", "script_name", "extra_capability"),
    [
        (
            "mutsukicore-codex-core",
            "MutsukiCore Codex Core",
            "mutsukicore-agent",
            "mutsukicore_codex_strategy_backend.py",
            None,
        ),
        (
            "mutsukicore-claude-core",
            "MutsukiCore Claude Core",
            "mutsukicore-claude-core",
            "smoke_bridge.py",
            "Claude",
        ),
    ],
)
def test_strategy_core_manifests_expose_strategy_skill_without_mcp_tools(
    plugin_name: str,
    display_name: str,
    skill_dir: str,
    script_name: str,
    extra_capability: str | None,
) -> None:
    plugin_path = _repo_root() / ".agents" / "plugins" / "plugins" / plugin_name
    manifest = _read_json(plugin_path / ".codex-plugin" / "plugin.json")

    assert manifest["name"] == plugin_name
    assert manifest["interface"]["displayName"] == display_name
    assert manifest["interface"]["category"] == "Productivity"
    assert "Strategy backend" in manifest["interface"]["capabilities"]
    if extra_capability is not None:
        assert extra_capability in manifest["interface"]["capabilities"]
    assert "mcpServers" not in manifest
    skill = plugin_path / "skills" / skill_dir / "SKILL.md"
    script = plugin_path / "scripts" / script_name
    assert skill.is_file()
    assert script.is_file()
