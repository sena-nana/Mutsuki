from __future__ import annotations

import pytest

from mutsukicore_runtime_python.backend import BackendInvokeError
from mutsukicore_runtime_python.contracts import (
    ERR_RUNTIME_BACKEND_FAILED,
    ERR_RUNTIME_BACKEND_GENERATION_MISMATCH,
    Envelope,
    JsonValue,
    OperationDescriptor,
    OperationStatus,
    SourceDescriptor,
    SourceRef,
    SourceSnapshot,
    StrategyResultStatus,
    to_json_dict,
)
from mutsukicore_runtime_python.host import PythonBackendHost
from mutsukicore_runtime_python.testing import WaitInputStrategy, run_backend_host_smoke


def _source_snapshot() -> SourceSnapshot:
    return SourceSnapshot(
        descriptor=SourceDescriptor(
            source_id="source:test",
            kind="test",
            capabilities=(),
            description="Test source",
        ),
        plugin_id="test-plugin",
        plugin_generation=0,
    )


def _descriptor() -> OperationDescriptor:
    return OperationDescriptor(
        op_id="test-plugin.echo",
        name="echo",
        plugin_id="test-plugin",
        parameters_schema={"type": "object"},
        return_schema={"type": "object"},
    )


def _envelope() -> Envelope:
    return Envelope(
        id="env-1",
        timestamp=1.0,
        source=SourceRef(source_id="source:test", kind="test"),
        payload_schema_id="test.input",
        payload={"value": "hello"},
    )


async def test_python_backend_host_runs_native_host_equivalent_smoke() -> None:
    host = PythonBackendHost()
    strategy = WaitInputStrategy()
    host.register_agent("agent-a", strategy)
    host.register_source(_source_snapshot())
    host.register_operation(_descriptor(), lambda payload: payload)

    result = await run_backend_host_smoke(
        host,
        agent_id="agent-a",
        envelope=_envelope(),
        op_id="test-plugin.echo",
        payload={"value": "ok"},
    )

    assert result == {"value": "ok"}
    assert strategy.awake_calls == 1
    assert strategy.stop_calls == 1
    assert strategy.inputs == [_envelope()]
    assert host.awake_count("agent-a") == 1
    assert host.stop_count("agent-a") == 1
    assert host.received_inputs("agent-a") == (_envelope(),)


async def test_python_backend_host_next_step_waits_for_input_by_default() -> None:
    host = PythonBackendHost()
    host.register_agent("agent-a")

    await host.on_awake("agent-a")
    result = await host.next_step("agent-a")

    assert result.status == StrategyResultStatus.WAIT_INPUT


async def test_operation_snapshot_serialization_does_not_expose_callable() -> None:
    host = PythonBackendHost()
    host.register_agent("agent-a")
    host.register_operation(_descriptor(), lambda payload: payload)

    snapshot = host.list_operations("agent-a")[0]
    encoded = to_json_dict(snapshot)
    encoded_key = encoded["key"]

    assert isinstance(encoded_key, dict)
    assert encoded_key["op_id"] == "test-plugin.echo"
    assert "handler" not in encoded
    assert "callable" not in str(encoded)


async def test_operation_status_rejects_stale_key_after_generation_advance() -> None:
    host = PythonBackendHost()
    host.register_agent("agent-a")
    stale_snapshot = host.register_operation(_descriptor(), lambda payload: payload)
    host.advance_plugin_generation("test-plugin")

    assert host.operation_status("agent-a", stale_snapshot.key) == OperationStatus.NOT_FOUND
    with pytest.raises(BackendInvokeError) as exc_info:
        await host.invoke("agent-a", stale_snapshot.key, {"value": "stale"})

    assert exc_info.value.error.code == ERR_RUNTIME_BACKEND_GENERATION_MISMATCH
    assert exc_info.value.error.evidence["expected_generation"] == 1
    assert exc_info.value.error.evidence["actual_generation"] == 0


async def test_handler_exception_is_wrapped_as_structured_backend_failure() -> None:
    def crash(_payload: JsonValue) -> JsonValue:
        raise ValueError("boom")

    host = PythonBackendHost()
    host.register_agent("agent-a")
    snapshot = host.register_operation(_descriptor(), crash)

    with pytest.raises(BackendInvokeError) as exc_info:
        await host.invoke("agent-a", snapshot.key, {})

    assert exc_info.value.error.code == ERR_RUNTIME_BACKEND_FAILED
    assert exc_info.value.error.evidence["exception_type"] == "ValueError"
    assert "boom" in str(exc_info.value.error.evidence["exception_repr"])


async def test_async_operation_handler_is_supported() -> None:
    async def echo(payload: JsonValue) -> JsonValue:
        return {"wrapped": payload}

    host = PythonBackendHost()
    host.register_agent("agent-a")
    snapshot = host.register_operation(_descriptor(), echo)

    result = await host.invoke("agent-a", snapshot.key, {"value": "ok"})

    assert result == {"wrapped": {"value": "ok"}}
