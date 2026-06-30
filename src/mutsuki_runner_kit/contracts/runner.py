from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass, field
from enum import StrEnum
from typing import Self

from mutsuki_runner_kit.contracts.codec import (
    JsonDict,
    ScalarValue,
    as_bool,
    as_int,
    as_json_dict,
    as_mapping,
    as_scalar_dict,
    as_str,
    as_str_tuple,
    field_value,
    optional_int,
    tuple_from_json,
)
from mutsuki_runner_kit.contracts.effect import EffectRequest
from mutsuki_runner_kit.contracts.event import DomainEvent
from mutsuki_runner_kit.contracts.resource import ResourceRef, ValueRef
from mutsuki_runner_kit.contracts.state import StateDelta
from mutsuki_runner_kit.contracts.task import Task, TaskAwait


class RunnerPurity(StrEnum):
    PURE = "pure"
    COMMITTER = "committer"
    EFFECTFUL = "effectful"


class ExecutionClass(StrEnum):
    CONTROL = "control"
    ORCHESTRATION = "orchestration"
    IO = "io"
    CPU = "cpu"
    BLOCKING = "blocking"
    SCRIPT = "script"


class RunnerStatus(StrEnum):
    COMPLETED = "completed"
    WAITING = "waiting"
    BLOCKED = "blocked"
    CONTINUE = "continue"
    FAILED = "failed"
    CANCELLED = "cancelled"


@dataclass(frozen=True)
class RunnerDescriptor:
    runner_id: str
    plugin_id: str
    plugin_generation: int
    accepted_protocol_ids: tuple[str, ...]
    purity: RunnerPurity
    execution_class: ExecutionClass
    input_schema: JsonDict = field(default_factory=dict)
    output_schema: JsonDict = field(default_factory=dict)
    metadata: dict[str, ScalarValue] = field(default_factory=dict)
    contract_surfaces: tuple[str, ...] = ()

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "RunnerDescriptor")
        return cls(
            runner_id=as_str(field_value(raw, "runner_id"), "runner_id"),
            plugin_id=as_str(field_value(raw, "plugin_id"), "plugin_id"),
            plugin_generation=as_int(field_value(raw, "plugin_generation"), "plugin_generation"),
            accepted_protocol_ids=as_str_tuple(
                field_value(raw, "accepted_protocol_ids"), "accepted_protocol_ids"
            ),
            purity=RunnerPurity(as_str(field_value(raw, "purity"), "purity")),
            execution_class=ExecutionClass(
                as_str(field_value(raw, "execution_class"), "execution_class")
            ),
            input_schema=as_json_dict(field_value(raw, "input_schema"), "input_schema"),
            output_schema=as_json_dict(field_value(raw, "output_schema"), "output_schema"),
            metadata=as_scalar_dict(field_value(raw, "metadata"), "metadata"),
            contract_surfaces=as_str_tuple(
                field_value(raw, "contract_surfaces"), "contract_surfaces"
            ),
        )


@dataclass(frozen=True)
class RunnerContext:
    registry_generation: int
    current_step: int
    executor_id: str
    task_lease_id: str | None
    invocation_id: str = ""
    cancel_token: str = ""
    deadline_tick: int | None = None
    cancel_requested: bool = False

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "RunnerContext")
        task_lease_id = field_value(raw, "task_lease_id")
        return cls(
            registry_generation=as_int(
                field_value(raw, "registry_generation"), "registry_generation"
            ),
            current_step=as_int(field_value(raw, "current_step"), "current_step"),
            executor_id=as_str(field_value(raw, "executor_id"), "executor_id"),
            task_lease_id=None
            if task_lease_id is None
            else as_str(task_lease_id, "task_lease_id"),
            invocation_id=as_str(field_value(raw, "invocation_id"), "invocation_id"),
            cancel_token=as_str(field_value(raw, "cancel_token"), "cancel_token"),
            deadline_tick=optional_int(field_value(raw, "deadline_tick"), "deadline_tick"),
            cancel_requested=as_bool(
                field_value(raw, "cancel_requested"), "cancel_requested"
            ),
        )


@dataclass(frozen=True)
class RunnerResult:
    task_id: str
    deltas: tuple[StateDelta, ...] = ()
    events: tuple[DomainEvent, ...] = ()
    tasks: tuple[Task, ...] = ()
    effects: tuple[EffectRequest, ...] = ()
    values: tuple[ValueRef, ...] = ()
    resources: tuple[ResourceRef, ...] = ()
    task_await: TaskAwait | None = None
    status: RunnerStatus = RunnerStatus.COMPLETED

    @classmethod
    def completed(cls, task_id: str) -> Self:
        return cls(task_id=task_id)

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "RunnerResult")
        task_await = field_value(raw, "task_await")
        return cls(
            task_id=as_str(field_value(raw, "task_id"), "task_id"),
            deltas=tuple_from_json(raw, "deltas", StateDelta),
            events=tuple_from_json(raw, "events", DomainEvent),
            tasks=tuple_from_json(raw, "tasks", Task),
            effects=tuple_from_json(raw, "effects", EffectRequest),
            values=tuple_from_json(raw, "values", ValueRef),
            resources=tuple_from_json(raw, "resources", ResourceRef),
            task_await=None
            if task_await is None
            else TaskAwait.from_json_dict(as_mapping(task_await, "task_await")),
            status=RunnerStatus(as_str(field_value(raw, "status"), "status")),
        )
