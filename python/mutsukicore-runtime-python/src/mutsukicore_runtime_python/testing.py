from __future__ import annotations

from mutsukicore_runtime_python.contracts import (
    Envelope,
    JsonValue,
    StrategyResult,
    from_json_dict,
    to_json_dict,
)
from mutsukicore_runtime_python.host import PythonBackendHost


class WaitInputStrategy:
    def __init__(self) -> None:
        self.awake_calls = 0
        self.stop_calls = 0
        self.inputs: list[Envelope] = []

    async def on_awake(self, agent_id: str) -> None:
        _ = agent_id
        self.awake_calls += 1

    async def on_input(self, agent_id: str, envelope: Envelope) -> StrategyResult:
        _ = agent_id
        self.inputs.append(envelope)
        return StrategyResult.wait_input()

    async def next_step(self, agent_id: str) -> StrategyResult:
        _ = agent_id
        return StrategyResult.wait_input()

    async def on_stop(self, agent_id: str) -> None:
        _ = agent_id
        self.stop_calls += 1


def assert_json_roundtrip[T](contract_type: type[T], value: T) -> T:
    encoded = to_json_dict(value)
    decoded = from_json_dict(contract_type, encoded)
    assert decoded == value
    return decoded


async def run_backend_host_smoke(
    host: PythonBackendHost,
    *,
    agent_id: str,
    envelope: Envelope,
    op_id: str,
    payload: JsonValue = None,
) -> JsonValue:
    await host.on_awake(agent_id)
    await host.on_input(agent_id, envelope)
    snapshot = next(item for item in host.list_operations(agent_id) if item.key.op_id == op_id)
    result = await host.invoke(agent_id, snapshot.key, payload)
    await host.on_stop(agent_id)
    return result


__all__ = [
    "WaitInputStrategy",
    "assert_json_roundtrip",
    "run_backend_host_smoke",
]
