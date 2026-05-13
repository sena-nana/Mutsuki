"""Agent 生命周期阶段。"""

from enum import StrEnum


class LifecyclePhase(StrEnum):
    SPAWN = "spawn"
    AWAKE = "awake"
    SLEEP = "sleep"
    STOP = "stop"


__all__ = ["LifecyclePhase"]
