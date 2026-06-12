"""Agent tick 调度器。

Core scheduler only owns lifecycle and generic Envelope fan-out to plugins.
Protocol-specific behavior such as IM text command parsing lives in reference
extensions (for example :mod:`mutsuki_ext.command`).

Graceful shutdown：``stop()`` 把一个 sentinel 放入 inbox，让 ``_loop``
处理完手头消息后自然退出，而不是直接 ``cancel()`` 打断正在执行的命令。
仅在 ``shutdown_timeout`` 超时后才回退到强制取消，作为最后兜底。
"""

from __future__ import annotations

import asyncio
from typing import TYPE_CHECKING, Final

from mutsuki.contracts.envelope import Envelope
from mutsuki.contracts.lifecycle import LifecyclePhase
from mutsuki.runtime.envelope_dispatch import dispatch_envelope_to_consumers

if TYPE_CHECKING:
    from mutsuki.core.agent import Agent


class _StopSentinel:
    """放入 inbox 用来通知 ``_loop`` 优雅退出的哨兵。"""


_STOP: Final[_StopSentinel] = _StopSentinel()


class AgentScheduler:
    def __init__(
        self,
        agent: "Agent",
        *,
        shutdown_timeout: float = 5.0,
    ) -> None:
        self.agent = agent
        self.shutdown_timeout = shutdown_timeout
        self._task: asyncio.Task[None] | None = None

    async def start(self) -> None:
        ctx = self.agent.make_context()
        self.agent.phase = LifecyclePhase.SLEEP
        await self.agent.lifespan.fire("awake", ctx)
        self.agent.phase = LifecyclePhase.AWAKE
        self._task = asyncio.create_task(self._loop())

    async def stop(self) -> None:
        if self._task is not None:
            # 优雅停机：让 _loop 处理完手头消息后自然退出。
            await self.agent.inbox.put(_STOP)
            try:
                await asyncio.wait_for(self._task, timeout=self.shutdown_timeout)
            except TimeoutError:
                # 超时兜底：强制取消（接受被打断命令的副作用半完成风险）
                self._task.cancel()
                try:
                    await self._task
                except asyncio.CancelledError:
                    pass
            # 真实 loop 异常不静默：让上层看到 bug。
        # sleep / stop 各自新建 ctx，避免 trace 上下文混淆两个阶段
        sleep_ctx = self.agent.make_context()
        self.agent.phase = LifecyclePhase.SLEEP
        await self.agent.lifespan.fire("sleep", sleep_ctx)
        stop_ctx = self.agent.make_context()
        self.agent.phase = LifecyclePhase.STOP
        await self.agent.lifespan.fire("stop", stop_ctx)
        await self.agent.close_agent_scope()

    async def _loop(self) -> None:
        # 直接阻塞 await，不再每秒 10 次轮询。stop 通过 _STOP sentinel 唤醒。
        while True:
            item = await self.agent.inbox.get()
            if item is _STOP:
                return
            if isinstance(item, Envelope):
                await self._dispatch_to_plugins(item)

    async def _dispatch_to_plugins(self, envelope: Envelope) -> None:
        """按 plugin.consumes 把 envelope 派发到所有匹配的 plugin。"""
        await dispatch_envelope_to_consumers(self.agent, envelope)

__all__ = ["AgentScheduler"]
