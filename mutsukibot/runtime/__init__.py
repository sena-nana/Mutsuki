"""运行时服务：决定性时钟、ID 生成器、RNG、调度器。

这些是注入到 ``AgentContext`` 中的运行时来源。插件**不得**直接调用
:func:`time.time` / :func:`uuid.uuid4` / :mod:`random`，必须通过 context。
"""

from mutsukibot.runtime.clock import (
    Clock,
    ManualClock,
    ManualClockWaiterOverflow,
    SystemClock,
)
from mutsukibot.runtime.idgen import DeterministicIdGen, IdGen, NanoIdGen
from mutsukibot.runtime.rng import RNG, SeededRng

__all__ = [
    "RNG",
    "Clock",
    "DeterministicIdGen",
    "IdGen",
    "ManualClock",
    "ManualClockWaiterOverflow",
    "NanoIdGen",
    "SeededRng",
    "SystemClock",
]
