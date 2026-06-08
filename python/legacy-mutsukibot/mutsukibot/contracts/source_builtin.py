"""框架内置的 SourceKind 常量门面。

:class:`SourceKinds` 让 pyright/IDE 能够准确推断核心 source kind 名。
插件采用相同模式自建门面（如
``McpKinds.FS = SourceKindName.register("mcp.fs", ...)``）。Core does not
pre-register protocol-specific source kinds; IM lives in ``mutsukibot_ext.im``.
"""

from __future__ import annotations

from mutsukibot.contracts.source import SourceKindName

_OWNER = "mutsukibot.core"


class SourceKinds:
    """所有 MutsukiBot 框架内置 source kind 常量。"""

    # Deprecated compatibility alias; canonical name lives in
    # ``mutsukibot_ext.im.IMSourceKinds``.
    IM: SourceKindName


SourceKindName.bootstrap_facade(SourceKinds, {}, declared_by=_OWNER)

from mutsukibot_ext.im import IMSourceKinds

SourceKinds.IM = IMSourceKinds.IM


__all__ = ["SourceKinds"]
