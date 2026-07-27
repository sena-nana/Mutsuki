from __future__ import annotations

import json
from collections.abc import Mapping
from importlib.resources import files
from typing import cast

import pytest

from mutsuki_runner_kit.contracts.batch import CompletionBatch, TaskBatch
from mutsuki_runner_kit.contracts.codec import JsonDict, JsonValue, from_json_dict, to_json_dict
from mutsuki_runner_kit.contracts.errors import RuntimeError
from mutsuki_runner_kit.contracts.resource import ResourceRef
from mutsuki_runner_kit.contracts.task import TaskHandle
from mutsuki_runner_kit.wire.generated import (
    CORE_WIRE_REVISION,
    MANAGEMENT_OPCODES,
    OPCODE_METHODS,
    Opcode,
)
from mutsuki_runner_kit.wire.handshake import ProtocolHelloAck
from mutsuki_runner_kit.wire.protocol import (
    BINARY_CODEC_ID,
    DEBUG_JSONL_CODEC_ID,
    SCHEMA_REVISION,
    ProtocolHello,
    WireProtocolFailure,
)
from mutsuki_runner_kit.wire.requests import (
    CancelRunnerRequest,
    DisposeRunnerRequest,
    InitializeRequest,
    RunBatchRequest,
    decode_request,
)
from mutsuki_runner_kit.wire.schema import RUNTIME_WIRE_SCHEMA


def test_generated_opcode_registry_matches_pinned_core_schema() -> None:
    operations = cast(list[dict[str, JsonValue]], RUNTIME_WIRE_SCHEMA["operations"])
    schema_registry = {
        _operation_opcode(operation): _operation_method(operation)
        for operation in operations
    }

    assert len(CORE_WIRE_REVISION) == 40
    assert RUNTIME_WIRE_SCHEMA["schema_revision"] == SCHEMA_REVISION
    assert RUNTIME_WIRE_SCHEMA["codecs"] == [DEBUG_JSONL_CODEC_ID, BINARY_CODEC_ID]
    assert schema_registry == {int(opcode): method for opcode, method in OPCODE_METHODS.items()}
    assert MANAGEMENT_OPCODES == frozenset(
        Opcode(_operation_opcode(operation))
        for operation in operations
        if operation["management"] is True
    )


def test_all_rust_generated_active_release_fixtures_roundtrip_in_python() -> None:
    artifact = json.loads(
        files("mutsuki_runner_kit.wire")
        .joinpath("runtime-wire-fixtures-v1.json")
        .read_text(encoding="utf-8")
    )
    fixtures = {item["type"]: item["value"] for item in artifact["fixtures"]}

    hello = ProtocolHello.from_mapping(_mapping(fixtures["ProtocolHello"]))
    assert hello.to_dict() == fixtures["ProtocolHello"]

    initialize_value = _mapping(fixtures["InitializeRequest"])
    initialize = decode_request(Opcode.PLUGIN_INITIALIZE, initialize_value)
    assert isinstance(initialize, InitializeRequest)
    assert initialize.hello == hello
    assert initialize.config == initialize_value["config"]

    ack = ProtocolHelloAck.from_mapping(_mapping(fixtures["ProtocolHelloAck"]))
    ack.validate_for(hello)
    assert ack.to_dict() == fixtures["ProtocolHelloAck"]

    run_value = _mapping(fixtures["RunBatchRequest"])
    run = decode_request(Opcode.RUNNER_RUN_BATCH, run_value)
    assert isinstance(run, RunBatchRequest)
    assert {
        "runner_id": run.runner_id,
        "ctx": to_json_dict(run.ctx),
        "batch": to_json_dict(run.batch),
    } == run_value

    completion = from_json_dict(CompletionBatch, _mapping(fixtures["CompletionBatch"]))
    assert to_json_dict(completion) == fixtures["CompletionBatch"]

    cancel_value = _mapping(fixtures["CancelRunnerRequest"])
    cancel = decode_request(Opcode.RUNNER_CANCEL, cancel_value)
    assert isinstance(cancel, CancelRunnerRequest)
    assert {"runner_id": cancel.runner_id, "invocation_id": cancel.invocation_id} == cancel_value

    dispose_value = _mapping(fixtures["DisposeRunnerRequest"])
    dispose = decode_request(Opcode.RUNNER_DISPOSE, dispose_value)
    assert isinstance(dispose, DisposeRunnerRequest)
    assert {"runner_id": dispose.runner_id} == dispose_value

    submit = _mapping(fixtures["SubmitTaskBatchRequest"])
    task_batch = from_json_dict(TaskBatch, _mapping(submit["batch"]))
    assert {"batch": to_json_dict(task_batch)} == submit

    for fixture_name, contract in (
        ("TaskHandle", TaskHandle),
        ("ResourceRef", ResourceRef),
        ("RuntimeError", RuntimeError),
    ):
        value = _mapping(fixtures[fixture_name])
        assert to_json_dict(from_json_dict(contract, value)) == value


def test_additive_payload_fields_do_not_break_typed_request_decode() -> None:
    request = decode_request(
        Opcode.RUNNER_CANCEL,
        {
            "runner_id": "runner-a",
            "invocation_id": "inv-1",
            "future_additive_field": {"enabled": True},
        },
    )

    assert request == CancelRunnerRequest("runner-a", "inv-1")


def test_handshake_rejects_missing_management_support_and_expanded_limits() -> None:
    hello = ProtocolHello.for_codec(DEBUG_JSONL_CODEC_ID)
    without_management = ProtocolHello(
        **{**hello.__dict__, "management_channel": False}
    )
    with pytest.raises(WireProtocolFailure, match="management channel"):
        ProtocolHelloAck.negotiate(without_management, DEBUG_JSONL_CODEC_ID)

    ack = ProtocolHelloAck.negotiate(hello, DEBUG_JSONL_CODEC_ID)
    expanded = ProtocolHelloAck(
        **{**ack.__dict__, "max_payload_bytes": hello.max_payload_bytes + 1}
    )
    with pytest.raises(WireProtocolFailure, match="expanded negotiated limits"):
        expanded.validate_for(hello)


def _mapping(value: object) -> Mapping[str, object] | JsonDict:
    if not isinstance(value, Mapping):
        raise TypeError("fixture value expects mapping")
    return value


def _operation_opcode(operation: dict[str, JsonValue]) -> int:
    value = operation["opcode"]
    if not isinstance(value, int) or isinstance(value, bool):
        raise TypeError("operation opcode expects int")
    return value


def _operation_method(operation: dict[str, JsonValue]) -> str:
    value = operation["method"]
    if not isinstance(value, str):
        raise TypeError("operation method expects str")
    return value
