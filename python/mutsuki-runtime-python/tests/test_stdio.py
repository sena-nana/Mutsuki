from __future__ import annotations

import json
from io import StringIO

from mutsuki_runtime_python.contracts import (
    ERR_REF_NOT_FOUND,
    ERR_RUNTIME_BACKEND_FAILED,
    ERR_RUNTIME_BACKEND_GENERATION_MISMATCH,
    Envelope,
    LeaseToken,
    OperationDescriptor,
    OperationStatus,
    RefDescriptor,
    SourceDescriptor,
    SourceRef,
    SourceSnapshot,
    to_json_dict,
)
from mutsuki_runtime_python.host import PythonBackendHost
from mutsuki_runtime_python.resource import CounterIdSource, PythonResourceBackend
from mutsuki_runtime_python.stdio import StdioJsonlBackendServer


def _dict_value(value: object) -> dict[str, object]:
    assert isinstance(value, dict)
    return value


def _resource_ref_id(record: object) -> object:
    return _dict_value(_dict_value(record)["descriptor"])["ref_id"]


def _host() -> PythonBackendHost:
    host = PythonBackendHost()
    host.register_agent("agent-a")
    host.register_source(
        SourceSnapshot(
            descriptor=SourceDescriptor(source_id="source:test", kind="test"),
            plugin_id="test-plugin",
            plugin_generation=0,
        )
    )
    host.register_operation(
        OperationDescriptor(op_id="test-plugin.echo", name="echo", plugin_id="test-plugin"),
        lambda payload: {"echo": payload},
    )
    return host


def _envelope() -> Envelope:
    return Envelope(
        id="env-1",
        timestamp=1.0,
        source=SourceRef(source_id="source:test", kind="test"),
        payload_schema_id="test.input",
        payload={"value": "hello"},
    )


def _resource_backend() -> PythonResourceBackend:
    return PythonResourceBackend(CounterIdSource())


def _descriptor() -> RefDescriptor:
    return RefDescriptor(
        ref_id="ref-1",
        kind="domain.resource",
        schema_id_target="domain.resource",
        schema_version_target="1.0.0",
    )


async def test_stdio_dispatches_host_methods() -> None:
    host = _host()
    server = StdioJsonlBackendServer(host, _resource_backend())
    snapshot = host.list_operations("agent-a")[0]

    assert await server.handle_request(
        {"id": "req-1", "method": "on_awake", "params": {"agent_id": "agent-a"}}
    ) == {"id": "req-1", "ok": True, "result": None}
    input_response = await server.handle_request(
        {
            "id": "req-2",
            "method": "on_input",
            "params": {"agent_id": "agent-a", "envelope": to_json_dict(_envelope())},
        }
    )
    assert input_response["ok"] is True
    invoke_response = await server.handle_request(
        {
            "id": "req-3",
            "method": "invoke",
            "params": {
                "agent_id": "agent-a",
                "key": to_json_dict(snapshot.key),
                "payload": {"value": "ok"},
            },
        }
    )

    assert invoke_response == {"id": "req-3", "ok": True, "result": {"echo": {"value": "ok"}}}


async def test_stdio_dispatches_resource_methods_and_rejects_forged_token() -> None:
    server = StdioJsonlBackendServer(_host(), _resource_backend())
    register = await server.handle_request(
        {
            "id": "req-1",
            "method": "resource.register",
            "params": {"descriptor": to_json_dict(_descriptor()), "owner": "resource-host"},
        }
    )
    acquire = await server.handle_request(
        {
            "id": "req-2",
            "method": "resource.acquire",
            "params": {"ref_id": "ref-1", "requester": "agent-a"},
        }
    )
    token = LeaseToken.from_json_dict(_dict_value(acquire["result"]))
    forged = LeaseToken(token_id=token.token_id, ref_id="ref-other", owner="agent-b")
    release = await server.handle_request(
        {
            "id": "req-3",
            "method": "resource.release",
            "params": {"token": to_json_dict(forged)},
        }
    )

    assert register == {"id": "req-1", "ok": True, "result": "ref-1"}
    assert release["ok"] is False
    release_error = _dict_value(release["error"])
    release_evidence = _dict_value(release_error["evidence"])
    assert release_error["code"] == ERR_REF_NOT_FOUND
    assert release_evidence["reason"] == "lease_token_mismatch"


async def test_stdio_rejects_unknown_method_and_malformed_request_structured() -> None:
    server = StdioJsonlBackendServer(_host(), _resource_backend())

    unknown = await server.handle_request({"id": "req-1", "method": "missing", "params": {}})
    malformed = await server.handle_request({"id": "req-2", "method": "invoke", "params": []})

    assert unknown["ok"] is False
    unknown_error = _dict_value(unknown["error"])
    unknown_evidence = _dict_value(unknown_error["evidence"])
    assert unknown_error["code"] == ERR_RUNTIME_BACKEND_FAILED
    assert unknown_evidence["reason"] == "unknown_method"
    assert malformed["ok"] is False
    malformed_error = _dict_value(malformed["error"])
    assert malformed_error["code"] == ERR_RUNTIME_BACKEND_FAILED


async def test_stdio_stale_operation_key_returns_generation_mismatch() -> None:
    host = _host()
    stale = host.list_operations("agent-a")[0]
    host.advance_plugin_generation("test-plugin")
    server = StdioJsonlBackendServer(host, _resource_backend())

    response = await server.handle_request(
        {
            "id": "req-1",
            "method": "invoke",
            "params": {
                "agent_id": "agent-a",
                "key": to_json_dict(stale.key),
                "payload": {"value": "stale"},
            },
        }
    )

    assert response["ok"] is False
    response_error = _dict_value(response["error"])
    assert response_error["code"] == ERR_RUNTIME_BACKEND_GENERATION_MISMATCH


async def test_stdio_operation_status_returns_snake_case_status() -> None:
    host = _host()
    snapshot = host.list_operations("agent-a")[0]
    server = StdioJsonlBackendServer(host, _resource_backend())

    response = await server.handle_request(
        {
            "id": "req-1",
            "method": "operation_status",
            "params": {"agent_id": "agent-a", "key": to_json_dict(snapshot.key)},
        }
    )

    assert response == {"id": "req-1", "ok": True, "result": OperationStatus.ACTIVE.value}


async def test_stdio_resource_list_supports_null_and_owner_filter() -> None:
    resource_backend = _resource_backend()
    await resource_backend.register_resource(_descriptor(), "owner-a")
    await resource_backend.register_resource(
        RefDescriptor(
            ref_id="ref-2",
            kind="domain.resource",
            schema_id_target="domain.resource",
            schema_version_target="1.0.0",
        ),
        "owner-b",
    )
    server = StdioJsonlBackendServer(_host(), resource_backend)

    all_response = await server.handle_request(
        {"id": "req-1", "method": "resource.list", "params": {"owner": None}}
    )
    owner_response = await server.handle_request(
        {"id": "req-2", "method": "resource.list", "params": {"owner": "owner-b"}}
    )

    assert all_response["ok"] is True
    all_records = all_response["result"]
    assert isinstance(all_records, list)
    assert [_resource_ref_id(record) for record in all_records] == ["ref-1", "ref-2"]
    assert owner_response["ok"] is True
    owner_records = owner_response["result"]
    assert isinstance(owner_records, list)
    assert len(owner_records) == 1
    assert _resource_ref_id(owner_records[0]) == "ref-2"


async def test_stdio_serve_reads_and_writes_jsonl() -> None:
    server = StdioJsonlBackendServer(_host(), _resource_backend())
    input_stream = StringIO(
        '{"id":"req-1","method":"next_step","params":{"agent_id":"agent-a"}}\n'
    )
    output_stream = StringIO()

    await server.serve(input_stream, output_stream)

    assert '"id":"req-1"' in output_stream.getvalue()
    assert '"ok":true' in output_stream.getvalue()


async def test_stdio_serve_malformed_json_decode_returns_structured_error() -> None:
    server = StdioJsonlBackendServer(_host(), _resource_backend())
    input_stream = StringIO('{"id":"req-1","method":\n')
    output_stream = StringIO()

    await server.serve(input_stream, output_stream)

    response = _dict_value(json.loads(output_stream.getvalue()))
    error = _dict_value(response["error"])
    assert response["id"] is None
    assert response["ok"] is False
    assert error["code"] == ERR_RUNTIME_BACKEND_FAILED
