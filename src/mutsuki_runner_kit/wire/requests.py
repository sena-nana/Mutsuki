from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass

from mutsuki_runner_kit.contracts.batch import WorkBatch
from mutsuki_runner_kit.contracts.codec import as_str, field_value, from_json_dict
from mutsuki_runner_kit.contracts.resource import (
    CommandBatch,
    CommandPlan,
    ExportPlan,
    SagaPlan,
)
from mutsuki_runner_kit.contracts.runner import RunnerContext
from mutsuki_runner_kit.wire.generated import Opcode
from mutsuki_runner_kit.wire.protocol import ProtocolHello, WireProtocolFailure


@dataclass(frozen=True)
class InitializeRequest:
    hello: ProtocolHello


@dataclass(frozen=True)
class RunBatchRequest:
    runner_id: str
    ctx: RunnerContext
    batch: WorkBatch


@dataclass(frozen=True)
class CancelRunnerRequest:
    runner_id: str
    invocation_id: str


@dataclass(frozen=True)
class DisposeRunnerRequest:
    runner_id: str


@dataclass(frozen=True)
class ExportPlanRequest:
    provider_id: str | None
    plan: ExportPlan


@dataclass(frozen=True)
class CommandPlanRequest:
    provider_id: str | None
    plan: CommandPlan


@dataclass(frozen=True)
class CommandBatchRequest:
    provider_id: str | None
    batch: CommandBatch


@dataclass(frozen=True)
class SagaPlanRequest:
    provider_id: str | None
    saga: SagaPlan


RunnerWireRequest = (
    InitializeRequest
    | RunBatchRequest
    | CancelRunnerRequest
    | DisposeRunnerRequest
    | ExportPlanRequest
    | CommandPlanRequest
    | CommandBatchRequest
    | SagaPlanRequest
)


def decode_request(opcode: Opcode, payload: Mapping[str, object]) -> RunnerWireRequest:
    if opcode is Opcode.PLUGIN_INITIALIZE:
        return InitializeRequest(
            ProtocolHello.from_mapping(_mapping_field(payload, "hello"))
        )
    if opcode is Opcode.RUNNER_RUN_BATCH:
        return RunBatchRequest(
            runner_id=as_str(field_value(payload, "runner_id"), "runner_id"),
            ctx=from_json_dict(RunnerContext, _mapping_field(payload, "ctx")),
            batch=from_json_dict(WorkBatch, _mapping_field(payload, "batch")),
        )
    if opcode is Opcode.RUNNER_CANCEL:
        return CancelRunnerRequest(
            runner_id=as_str(field_value(payload, "runner_id"), "runner_id"),
            invocation_id=as_str(
                field_value(payload, "invocation_id"), "invocation_id"
            ),
        )
    if opcode is Opcode.RUNNER_DISPOSE:
        return DisposeRunnerRequest(
            runner_id=as_str(field_value(payload, "runner_id"), "runner_id")
        )
    provider_id = _optional_str(payload.get("provider_id"), "provider_id")
    if opcode is Opcode.RESOURCE_EXPORT:
        return ExportPlanRequest(
            provider_id, from_json_dict(ExportPlan, _mapping_field(payload, "plan"))
        )
    if opcode is Opcode.RESOURCE_COMMAND:
        return CommandPlanRequest(
            provider_id, from_json_dict(CommandPlan, _mapping_field(payload, "plan"))
        )
    if opcode is Opcode.RESOURCE_COMMAND_BATCH:
        return CommandBatchRequest(
            provider_id,
            from_json_dict(CommandBatch, _mapping_field(payload, "batch")),
        )
    if opcode is Opcode.RESOURCE_SAGA:
        return SagaPlanRequest(
            provider_id, from_json_dict(SagaPlan, _mapping_field(payload, "saga"))
        )
    raise WireProtocolFailure(
        "wire.opcode_unsupported",
        f"opcode {int(opcode):#06x} is not served by the Python runner endpoint",
    )


def _mapping_field(payload: Mapping[str, object], name: str) -> Mapping[str, object]:
    value = field_value(payload, name)
    if not isinstance(value, Mapping):
        raise TypeError(f"{name} expects mapping")
    return value


def _optional_str(value: object, name: str) -> str | None:
    return None if value is None else as_str(value, name)

