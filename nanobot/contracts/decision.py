"""Decision 契约 —— 记录「选了什么、为什么」。"""

from __future__ import annotations

from typing import ClassVar

import msgspec

from nanobot.contracts.base import Contract


class Decision(Contract):
    """已记录的决策（路由、payload、备选项）。"""

    schema_id: ClassVar[str] = "nanobot.decision"
    schema_version: ClassVar[str] = "1.0.0"

    id: str
    source: str
    route: str
    payload: msgspec.Raw
    alternatives_considered: tuple[str, ...] = ()


__all__ = ["Decision"]
