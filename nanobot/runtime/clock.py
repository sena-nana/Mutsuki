"""Clock 协议与内置实现。"""

from __future__ import annotations

import asyncio
import heapq
import itertools
import time
from typing import Protocol, runtime_checkable
import warnings


@runtime_checkable
class Clock(Protocol):
    """墙钟 + 单调时间源。插件必须用此接口，不能直接用 :mod:`time`。"""

    def now(self) -> float: ...
    def monotonic(self) -> float: ...
    async def sleep(self, seconds: float) -> None: ...


class SystemClock:
    """生产环境时钟，由 :mod:`time` 与 :mod:`asyncio` 支撑。"""

    def now(self) -> float:
        return time.time()

    def monotonic(self) -> float:
        return time.monotonic()

    async def sleep(self, seconds: float) -> None:
        await asyncio.sleep(seconds)


class ManualClockWaiterOverflow(RuntimeWarning):
    """挂起的 sleeper 数超过阈值时发出，提示测试可能忘记 advance。"""


class ManualClock:
    """测试用时钟 —— 墙钟与单调时间只在主动推进时前进。

    ``sleep()`` 会一直阻塞，直到通过 :meth:`advance` 推进了足够的虚拟时间。
    内部用 min-heap 维护待唤醒队列，``advance`` 摊销 O(k log n)。
    超出 ``max_pending_waiters`` 时通过 :class:`ManualClockWaiterOverflow`
    告警，避免测试因忘记 ``advance`` 静默泄漏。
    """

    def __init__(self, start: float = 0.0, *, max_pending_waiters: int = 1024) -> None:
        self._wall = start
        self._mono = 0.0
        # heap entries: (deadline, seq, event)；seq 保证同 deadline 时 FIFO
        # 且让 (deadline, ...) 之间永远可比较。
        self._waiters: list[tuple[float, int, asyncio.Event]] = []
        self._counter = itertools.count()
        self._max_pending_waiters = max_pending_waiters

    def now(self) -> float:
        return self._wall

    def monotonic(self) -> float:
        return self._mono

    async def sleep(self, seconds: float) -> None:
        if seconds <= 0:
            return
        target = self._mono + seconds
        evt = asyncio.Event()
        heapq.heappush(self._waiters, (target, next(self._counter), evt))
        if len(self._waiters) > self._max_pending_waiters:
            warnings.warn(
                f"ManualClock 挂起 sleeper 数 {len(self._waiters)} 超过阈值 "
                f"{self._max_pending_waiters}，可能忘记 advance() 或测试泄漏。",
                ManualClockWaiterOverflow,
                stacklevel=2,
            )
        await evt.wait()

    def advance(self, seconds: float) -> None:
        """推进虚拟时间，唤醒所有已经到期的 sleeper。"""
        self._mono += seconds
        self._wall += seconds
        while self._waiters and self._waiters[0][0] <= self._mono:
            _deadline, _seq, evt = heapq.heappop(self._waiters)
            evt.set()

    @property
    def pending_waiters(self) -> int:
        return len(self._waiters)

    def cancel_all(self) -> int:
        """唤醒所有还在挂起的 sleeper（用于测试 teardown），返回被唤醒的数量。"""
        count = len(self._waiters)
        for _deadline, _seq, evt in self._waiters:
            evt.set()
        self._waiters.clear()
        return count


__all__ = ["Clock", "ManualClock", "ManualClockWaiterOverflow", "SystemClock"]
