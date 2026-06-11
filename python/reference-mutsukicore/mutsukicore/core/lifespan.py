"""生命周期钩子列表（NoneBot 风格的 ``Lifespan``）。

Agent 生命周期对应三组独立列表：

* ``on_awake`` —— 调度器启动，开始接受命令
* ``on_sleep`` —— 调度器暂停，命令排队或拒绝
* ``on_stop`` —— 调度器停止，插件卸载完毕

钩子按声明顺序登记；``on_sleep`` / ``on_stop`` 反向执行以实现 LIFO 退栈。

原 ``on_spawn`` 钩子在 v0.1 没有任何触发点（``Plugin.on_load`` 已覆盖
"加载后初始化"窗口），保留即隐藏 bug，故删除。
"""

from __future__ import annotations

from collections.abc import Awaitable, Callable
from dataclasses import dataclass, field
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from mutsukicore.core.context import AgentContext

Hook = Callable[["AgentContext"], Awaitable[None]]


@dataclass(slots=True)
class Lifespan:
    on_awake: list[Hook] = field(default_factory=list)
    on_sleep: list[Hook] = field(default_factory=list)
    on_stop: list[Hook] = field(default_factory=list)

    async def fire(self, phase: str, ctx: "AgentContext") -> None:
        hooks: list[Hook]
        if phase == "awake":
            hooks = self.on_awake
        elif phase == "sleep":
            hooks = list(reversed(self.on_sleep))
        elif phase == "stop":
            hooks = list(reversed(self.on_stop))
        else:
            raise ValueError(f"未知的生命周期阶段: {phase!r}")
        for hook in hooks:
            await hook(ctx)


__all__ = ["Hook", "Lifespan"]
