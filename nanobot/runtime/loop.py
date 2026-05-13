"""异步运行时门面。

当前是 :func:`asyncio.run` 的薄包装。计划中提到的「同步点检测钩子」
（捕获插件调用栈里的 ``time.sleep`` 等）位于特性开关后，仅在测试中启用。
"""

from __future__ import annotations

import asyncio
from collections.abc import Awaitable, Callable
from typing import TypeVar

T = TypeVar("T")


def run(coro: Awaitable[T]) -> T:
    return asyncio.run(coro)  # type: ignore[arg-type]


def gather(*coros: Awaitable[object]) -> Awaitable[list[object]]:
    return asyncio.gather(*coros)


def create_task(coro: Awaitable[T]) -> asyncio.Task[T]:
    return asyncio.create_task(coro)  # type: ignore[arg-type]


def install_sync_point_guard(_callback: Callable[[str], None]) -> None:
    """v0.1 占位 —— 完整的基于 sys.settrace 的检测在 v0.2 落地。

    v0.1 阶段我们依靠 ruff 静态规则，禁止插件代码使用
    ``time.sleep`` / ``time.time`` / ``uuid.uuid4`` / 全局 ``random``。
    """


__all__ = ["create_task", "gather", "install_sync_point_guard", "run"]
