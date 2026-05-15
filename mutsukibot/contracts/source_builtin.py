"""框架内置的 SourceKind 常量门面。

:class:`SourceKinds` 让 pyright/IDE 能够准确推断核心 source kind 名。
插件采用相同模式自建门面（如
``McpKinds.FS = SourceKindName.register("mcp.fs", ...)``）。
"""

from __future__ import annotations

from typing import ClassVar

from mutsukibot.contracts.source import SourceKindName

_OWNER = "mutsukibot.core"


class SourceKinds:
    """所有 MutsukiBot 框架内置 source kind 常量。"""

    IM: ClassVar[SourceKindName]
    TOOL: ClassVar[SourceKindName]
    HYBRID: ClassVar[SourceKindName]


SourceKindName.bootstrap_facade(
    SourceKinds,
    {
        "IM": "im",
        "TOOL": "tool",
        "HYBRID": "hybrid",
    },
    declared_by=_OWNER,
)


__all__ = ["SourceKinds"]
