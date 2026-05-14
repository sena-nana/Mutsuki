"""随机数生成器协议 —— 可种子化、可重放。"""

from __future__ import annotations

import random
from typing import Protocol, runtime_checkable


@runtime_checkable
class RNG(Protocol):
    """面向插件的 RNG 接口。底层用 :class:`random.Random`。"""

    def random(self) -> float: ...
    def randint(self, a: int, b: int) -> int: ...
    def choice(self, seq: list[object]) -> object: ...


class SeededRng:
    """带固定种子的 ``random.Random`` 包装。"""

    def __init__(self, seed: int = 0) -> None:
        self._r = random.Random(seed)

    def random(self) -> float:
        return self._r.random()

    def randint(self, a: int, b: int) -> int:
        return self._r.randint(a, b)

    def choice(self, seq: list[object]) -> object:
        return self._r.choice(seq)


__all__ = ["RNG", "SeededRng"]
