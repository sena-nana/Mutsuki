"""Agent 生命周期阶段。

v0.1 简化为三阶段：``awake → sleep → stop``。原 ``SPAWN`` 阶段在 v0.1
代码中没有任何触发点，``Plugin.on_load`` 已经覆盖"加载后初始化"窗口；
保留一个永远不会被消费的钩子等同于隐藏 bug，故删除。

如果未来需要"Agent 创建钩子"，应通过 AgentRegistry 的全局钩子表达，
而不是回到每个 Agent 实例自带 ``on_spawn``。
"""

from enum import StrEnum


class LifecyclePhase(StrEnum):
    AWAKE = "awake"
    SLEEP = "sleep"
    STOP = "stop"


__all__ = ["LifecyclePhase"]
