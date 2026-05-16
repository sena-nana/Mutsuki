from __future__ import annotations

from pathlib import Path

from mutsukibot.testing.plugin_lint import lint_plugin_io_fields


def test_plugin_io_lint_rejects_raw_socket_field(tmp_path: Path) -> None:
    path = tmp_path / "bad_plugin.py"
    path.write_text(
        """
from typing import ClassVar
import msgspec
import socket
from mutsukibot import Capability, Caps, Plugin

class Config(msgspec.Struct, kw_only=True):
    pass

class BadPlugin(Plugin[Config]):
    id: ClassVar[str] = "bad-plugin"
    version: ClassVar[str] = "0.0.1"
    capabilities: ClassVar[list[Capability]] = [Capability(name=Caps.NETWORK_EGRESS)]
    Config = Config
    raw_socket: socket.socket | None = None
""",
        encoding="utf-8",
    )

    violations = lint_plugin_io_fields(path)

    assert len(violations) == 1
    assert violations[0].plugin_class == "BadPlugin"
    assert violations[0].field_name == "raw_socket"
    assert violations[0].raw_type == "socket.socket"


def test_plugin_io_lint_allows_handle_field(tmp_path: Path) -> None:
    path = tmp_path / "good_plugin.py"
    path.write_text(
        """
from typing import Any, ClassVar
import msgspec
from mutsukibot import Capability, Caps, Plugin
from mutsukibot.contracts import Handle

class Config(msgspec.Struct, kw_only=True):
    pass

class GoodPlugin(Plugin[Config]):
    id: ClassVar[str] = "good-plugin"
    version: ClassVar[str] = "0.0.1"
    capabilities: ClassVar[list[Capability]] = [Capability(name=Caps.NETWORK_EGRESS)]
    Config = Config
    ws_handle: Handle[Any] | None = None
""",
        encoding="utf-8",
    )

    assert lint_plugin_io_fields(path) == []


def test_repository_plugins_do_not_hold_raw_io_fields() -> None:
    violations = []
    for path in Path("mutsukibot/plugins").rglob("*.py"):
        violations.extend(lint_plugin_io_fields(path))

    assert violations == []

