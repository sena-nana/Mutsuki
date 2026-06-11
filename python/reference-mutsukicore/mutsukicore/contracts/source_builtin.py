"""框架内置的 SourceKind 常量门面。

:class:`SourceKinds` 让 pyright/IDE 能够准确推断核心 source kind 名。
插件采用相同模式自建门面（如
``McpKinds.FS = SourceKindName.register("mcp.fs", ...)``）。Core does not
pre-register protocol-specific source kinds; IM lives in ``mutsukicore_ext.im``.
"""

from __future__ import annotations

from mutsukicore.contracts.source import SourceKindName

_OWNER = "mutsukicore.core"


class SourceKinds:
    """所有 MutsukiCore 框架内置 source kind 常量。"""

    # Deprecated compatibility alias; canonical name lives in
    # ``mutsukicore_ext.im.IMSourceKinds``.
    IM: SourceKindName


SourceKindName.bootstrap_facade(SourceKinds, {}, declared_by=_OWNER)

from mutsukicore_ext.im import IMSourceKinds

SourceKinds.IM = IMSourceKinds.IM


__all__ = ["SourceKinds"]
