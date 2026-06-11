"""Small benchmark helpers used by regression tests."""

from __future__ import annotations

from dataclasses import dataclass
from time import perf_counter
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from mutsukicore.core.context import AgentContext
    from mutsukicore.core.dispatcher import Dispatcher


@dataclass(frozen=True, slots=True)
class DispatchInvokeStats:
    iterations: int
    mean_ms: float
    p50_ms: float
    p95_ms: float


async def measure_dispatcher_invoke(
    dispatcher: "Dispatcher",
    op_id: str,
    *,
    ctx: "AgentContext",
    iterations: int = 500,
    payload: dict[str, Any] | None = None,
) -> DispatchInvokeStats:
    """Measure dispatcher.invoke latency with a tiny in-process loop."""
    samples: list[float] = []
    for _ in range(iterations):
        start = perf_counter()
        await dispatcher.invoke(op_id, payload or {}, ctx=ctx)
        samples.append((perf_counter() - start) * 1000.0)

    ordered = sorted(samples)
    p50 = ordered[int((iterations - 1) * 0.50)]
    p95 = ordered[int((iterations - 1) * 0.95)]
    return DispatchInvokeStats(
        iterations=iterations,
        mean_ms=sum(samples) / iterations,
        p50_ms=p50,
        p95_ms=p95,
    )

