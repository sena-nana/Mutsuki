"""进程内 Agent 注册表。

v0.2 Phase C 用它承接多 Agent 协作：

* Agent 在创建时自动登记
* dispatcher.publish 通过 ``iter_accepting(envelope)`` 枚举所有匹配 Agent
* 处于 ``LifecyclePhase.STOP`` 的 Agent 不再接收 envelope

注册表使用弱引用保存 Agent，避免测试或短生命周期对象因为全局表而泄漏。
"""

from __future__ import annotations

from collections.abc import Callable, Iterator
from dataclasses import dataclass
from typing import TYPE_CHECKING
from weakref import WeakValueDictionary

from mutsukibot.contracts.lifecycle import LifecyclePhase
from mutsukibot.core.agent_election import (
    AgentElectionPolicy,
    PriorityThenIdElectionPolicy,
)

if TYPE_CHECKING:
    from mutsukibot.contracts.envelope import Envelope
    from mutsukibot.core.agent import Agent


@dataclass(frozen=True, slots=True)
class _ElectionPolicyEntry:
    token: object
    owner: str
    policy: AgentElectionPolicy


class _AgentRegistry:
    def __init__(self) -> None:
        self._agents: WeakValueDictionary[str, "Agent"] = WeakValueDictionary()
        self._default_policy: AgentElectionPolicy = PriorityThenIdElectionPolicy()
        self._policy_stack: list[_ElectionPolicyEntry] = []

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
        self._policy_stack.clear()

    def install_election_policy(
        self,
        policy: AgentElectionPolicy,
        *,
        owner: str,
    ) -> Callable[[], None]:
        """Install a process-local election policy and return an idempotent disposer.

        Later policies take precedence. Disposing the active policy restores the
        previous policy, which lets plugins attach the disposer to PluginScope
        without coordinating with other plugins.
        """

        token = object()
        self._policy_stack.append(
            _ElectionPolicyEntry(token=token, owner=owner, policy=policy)
        )
        disposed = False

        def _dispose() -> None:
            nonlocal disposed
            if disposed:
                return
            disposed = True
            self._policy_stack = [
                entry for entry in self._policy_stack if entry.token is not token
            ]

        return _dispose

    def iter_accepting(self, envelope: Envelope) -> Iterator[Agent]:
        yield from self.rank_accepting(envelope)

    def rank_accepting(self, envelope: Envelope) -> tuple[Agent, ...]:
        """返回所有匹配 envelope 的 awake Agent，按确定性优先级排序。

        默认策略提供 v0.3 MVP 的稳定排序：

        1. ``priority`` 高者优先
        2. priority 相同按 ``agent_id`` 字典序升序

        v0.3 后续允许插件通过 ``install_election_policy`` 替换排序策略；路由
        前置过滤仍由 registry 固定执行。
        """
        matched: list[Agent] = []
        for agent in tuple(self._agents.values()):
            if agent.phase != LifecyclePhase.AWAKE:
                continue
            accepts = agent.accepts
            if not accepts:
                continue
            if any(rule.check(envelope) for rule in accepts):
                matched.append(agent)
        return tuple(self._current_election_policy().rank(envelope, matched))

    def select_accepting(self, envelope: Envelope) -> Agent | None:
        """选出单个最佳匹配 Agent；无匹配时返回 ``None``。"""
        ranked = self.rank_accepting(envelope)
        return ranked[0] if ranked else None

    def _current_election_policy(self) -> AgentElectionPolicy:
        if self._policy_stack:
            return self._policy_stack[-1].policy
        return self._default_policy


AgentRegistry = _AgentRegistry()


__all__ = ["AgentRegistry"]
