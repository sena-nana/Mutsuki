"""Agent election policies.

Registry-level routing first filters by lifecycle and ``Agent.accepts``. An
election policy only orders the already-eligible candidates, so plugins can
replace winner selection without bypassing hard rule #13.
"""

from __future__ import annotations

from collections.abc import Sequence
from typing import TYPE_CHECKING, Protocol

if TYPE_CHECKING:
    from mutsukibot.contracts.envelope import Envelope
    from mutsukibot.core.agent import Agent


class AgentElectionPolicy(Protocol):
    """Orders eligible Agents for broadcast and single-winner selection."""

    def rank(
        self,
        envelope: "Envelope",
        candidates: Sequence["Agent"],
    ) -> tuple["Agent", ...]: ...


class PriorityThenIdElectionPolicy:
    """Default deterministic v0.3 election policy."""

    def rank(
        self,
        envelope: "Envelope",
        candidates: Sequence["Agent"],
    ) -> tuple["Agent", ...]:
        _ = envelope
        return tuple(
            sorted(
                candidates,
                key=lambda agent: (-agent.priority, str(agent.agent_id)),
            )
        )


__all__ = ["AgentElectionPolicy", "PriorityThenIdElectionPolicy"]
