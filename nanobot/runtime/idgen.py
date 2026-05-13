"""ID 生成协议与实现。

插件必须用 ``ctx.id_gen``，而不是直接调用 :func:`uuid.uuid4`，这样测试
才能注入决定性生成器复现 trace。
"""

from __future__ import annotations

import secrets
from typing import Protocol, runtime_checkable

_BASE32 = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"


@runtime_checkable
class IdGen(Protocol):
    """生成不透明、单调推进的字符串 ID。"""

    def next(self, prefix: str = "") -> str: ...


class NanoIdGen:
    """生产环境 ID 生成器 —— Crockford-base32 编码的随机 ID。

    格式：``<prefix>_<26 字符>``。前 10 位带时间因子用于粗粒度可排序，
    本身不作为安全令牌。
    """

    def __init__(self) -> None:
        self._counter = 0

    def next(self, prefix: str = "") -> str:
        self._counter += 1
        raw = secrets.token_bytes(16)
        body = "".join(_BASE32[b & 0x1F] for b in raw)[:26]
        return f"{prefix}_{body}" if prefix else body


class DeterministicIdGen:
    """测试用 ID 生成器 —— 决定性递增序列。"""

    def __init__(self, seed: int = 0) -> None:
        self._counter = seed

    def next(self, prefix: str = "") -> str:
        self._counter += 1
        body = f"{self._counter:026d}"
        return f"{prefix}_{body}" if prefix else body


__all__ = ["DeterministicIdGen", "IdGen", "NanoIdGen"]
