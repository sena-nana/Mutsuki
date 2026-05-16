"""进程内 Agent 注册表。

v0.2 Phase C 用它承接多 Agent 协作：

* Agent 在创建时自动登记
* dispatcher.publish 通过 ``iter_accepting(envelope)`` 枚举所有匹配 Agent
* 处于 ``LifecyclePhase.STOP`` 的 Agent 不再接收 envelope

注册表使用弱引用保存 Agent，避免测试或短生命周期对象因为全局表而泄漏。
"""

from __future__ import annotations

from collections.abc import Iterator
from typing import TYPE_CHECKING
from weakref import WeakValueDictionary

from mutsukibot.contracts.lifecycle import LifecyclePhase

if TYPE_CHECKING:
    from mutsukibot.contracts.envelope import Envelope
    from mutsukibot.core.agent import Agent


class _AgentRegistry:
    def __init__(self) -> None:
        self._agents: WeakValueDictionary[str, "Agent"] = WeakValueDictionary()

    def register(self, agent: "Agent") -> None:
        self._agents[str(agent.agent_id)] = agent

    def unregister(self, agent: Agent | str) -> None:
        agent_id = str(agent.agent_id) if not isinstance(agent, str) else agent
        self._agents.pop(agent_id, None)

    def get(self, agent_id: str) -> Agent | None:
        return self._agents.get(agent_id)

    def all(self) -> tuple["Agent", ...]:
        return tuple(self._agents.values())

    def clear(self) -> None:
        self._agents.clear()

    def iter_accepting(self, envelope: Envelope) -> Iterator[Agent]:
        for agent in tuple(self._agents.values()):
            if agent.phase != LifecyclePhase.AWAKE:
                continue
            accepts = agent.accepts
            if not accepts:
                continue
            if any(rule.check(envelope) for rule in accepts):
                yield agent


AgentRegistry = _AgentRegistry()


__all__ = ["AgentRegistry"]
