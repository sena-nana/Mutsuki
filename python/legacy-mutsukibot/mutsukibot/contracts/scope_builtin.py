"""框架内置 ScopeName 常量门面。

:class:`Scopes` 暴露 MutsukiBot 核心常用 ScopeRule 命名集合。插件采用相同
模式自建门面（如
``YumeScopes.THOUGHT = ScopeName.register("yume.thought", ..., rule=...)``）。
"""

from __future__ import annotations

from mutsukibot.contracts.scope import ScopeName

_OWNER = "mutsukibot.core"


class Scopes:
    """所有 MutsukiBot 框架内置 ScopeName 常量。"""

    # Deprecated compatibility aliases; canonical names live in
    # ``mutsukibot_ext.im.IMScopes``.
    IM_TEXT: ScopeName
    IM_ANY: ScopeName


ScopeName.bootstrap_facade(Scopes, {}, declared_by=_OWNER)

from mutsukibot_ext.im import IMScopes

Scopes.IM_TEXT = IMScopes.TEXT
Scopes.IM_ANY = IMScopes.ANY

__all__ = ["Scopes"]
