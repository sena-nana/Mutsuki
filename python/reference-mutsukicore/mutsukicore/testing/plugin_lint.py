"""Static checks for plugin I/O ownership rules.

Hard rule #14 forbids Plugin instances from storing raw sockets, SDK clients,
or transport connections directly. Plugins should attach those resources to a
PluginScope through Handle[T] so reload and resource lifetime stay decoupled.
"""

from __future__ import annotations

import ast
from dataclasses import dataclass
from pathlib import Path

_RAW_IO_TYPES = (
    "socket.socket",
    "aiohttp.ClientSession",
    "websockets.WebSocketServerProtocol",
    "websockets.WebSocketClientProtocol",
    "websockets.asyncio.server.Server",
    "websockets.asyncio.server.ServerConnection",
    "websockets.asyncio.client.ClientConnection",
    "ServerConnection",
    "ClientConnection",
    "WebSocketServerProtocol",
    "WebSocketClientProtocol",
)


@dataclass(frozen=True, slots=True)
class PluginIoFieldViolation:
    path: Path
    plugin_class: str
    field_name: str
    raw_type: str
    line: int


def lint_plugin_io_fields(path: str | Path) -> list[PluginIoFieldViolation]:
    """Return raw-I/O field violations in Plugin subclasses defined in path."""
    source_path = Path(path)
    tree = ast.parse(source_path.read_text(encoding="utf-8"), filename=str(source_path))
    violations: list[PluginIoFieldViolation] = []

    for node in tree.body:
        if not isinstance(node, ast.ClassDef) or not _is_plugin_subclass(node):
            continue
        for stmt in node.body:
            if isinstance(stmt, ast.AnnAssign):
                field_name = _field_name(stmt.target)
                if field_name is None or stmt.annotation is None:
                    continue
                raw_type = _raw_type_in_annotation(ast.unparse(stmt.annotation))
                if raw_type is not None:
                    violations.append(
                        PluginIoFieldViolation(
                            path=source_path,
                            plugin_class=node.name,
                            field_name=field_name,
                            raw_type=raw_type,
                            line=stmt.lineno,
                        )
                    )
            elif isinstance(stmt, ast.FunctionDef | ast.AsyncFunctionDef):
                violations.extend(_lint_instance_body(source_path, node.name, stmt))
    return violations


def _lint_instance_body(
    path: Path,
    plugin_class: str,
    fn: ast.FunctionDef | ast.AsyncFunctionDef,
) -> list[PluginIoFieldViolation]:
    violations: list[PluginIoFieldViolation] = []
    for stmt in ast.walk(fn):
        if isinstance(stmt, ast.AnnAssign):
            field_name = _field_name(stmt.target)
            if field_name is None or stmt.annotation is None:
                continue
            raw_type = _raw_type_in_annotation(ast.unparse(stmt.annotation))
            if raw_type is not None:
                violations.append(
                    PluginIoFieldViolation(
                        path=path,
                        plugin_class=plugin_class,
                        field_name=field_name,
                        raw_type=raw_type,
                        line=stmt.lineno,
                    )
                )
        elif isinstance(stmt, ast.Assign):
            for target in stmt.targets:
                field_name = _field_name(target)
                if field_name is None:
                    continue
                raw_type = _raw_type_in_constructor(stmt.value)
                if raw_type is not None:
                    violations.append(
                        PluginIoFieldViolation(
                            path=path,
                            plugin_class=plugin_class,
                            field_name=field_name,
                            raw_type=raw_type,
                            line=stmt.lineno,
                        )
                    )
    return violations


def _is_plugin_subclass(node: ast.ClassDef) -> bool:
    return any(_base_mentions_plugin(base) for base in node.bases)


def _base_mentions_plugin(base: ast.expr) -> bool:
    text = ast.unparse(base)
    return text == "Plugin" or text.startswith("Plugin[") or text.endswith(".Plugin")


def _field_name(target: ast.expr) -> str | None:
    if isinstance(target, ast.Name):
        return target.id
    if (
        isinstance(target, ast.Attribute)
        and isinstance(target.value, ast.Name)
        and target.value.id == "self"
    ):
        return target.attr
    return None


def _raw_type_in_annotation(annotation: str) -> str | None:
    if "Handle[" in annotation or annotation == "Handle":
        return None
    return next((raw for raw in _RAW_IO_TYPES if raw in annotation), None)


def _raw_type_in_constructor(value: ast.expr) -> str | None:
    if not isinstance(value, ast.Call):
        return None
    func = ast.unparse(value.func)
    return next((raw for raw in _RAW_IO_TYPES if func == raw), None)
