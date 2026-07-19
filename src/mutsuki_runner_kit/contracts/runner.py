from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass, field
from enum import StrEnum
from typing import Self

from mutsuki_runner_kit.contracts.codec import (
    JsonDict,
    JsonValue,
    ScalarValue,
    as_bool,
    as_int,
    as_json_dict,
    as_json_value,
    as_mapping,
    as_scalar_dict,
    as_str,
    as_str_tuple,
    field_value,
    optional_int,
    tuple_from_json,
)
from mutsuki_runner_kit.contracts.effect import EffectRequest
from mutsuki_runner_kit.contracts.entry import OrderingRequirement, PayloadLayout
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


class InvocationMode(StrEnum):
    SYNC_EXCLUSIVE = "sync_exclusive"
    ASYNC_REENTRANT = "async_reentrant"
    ASYNC_EXCLUSIVE = "async_exclusive"
    EXTERNAL_PROCESS = "external_process"


class RunnerConcurrencyMode(StrEnum):
    EXCLUSIVE = "exclusive"
    REENTRANT = "reentrant"
    SHARDED = "sharded"


@dataclass(frozen=True)
class RunnerConcurrency:
    mode: RunnerConcurrencyMode = RunnerConcurrencyMode.EXCLUSIVE
    max_inflight_batches: int | None = None
    max_inflight_entries: int | None = None
    instances: int | None = None

    @classmethod
    def exclusive(cls) -> Self:
        return cls()

    @classmethod
    def reentrant(cls, max_inflight_batches: int, max_inflight_entries: int) -> Self:
        return cls(
            mode=RunnerConcurrencyMode.REENTRANT,
            max_inflight_batches=max_inflight_batches,
            max_inflight_entries=max_inflight_entries,
        )

    @classmethod
    def sharded(cls, instances: int) -> Self:
        return cls(mode=RunnerConcurrencyMode.SHARDED, instances=instances)

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "RunnerConcurrency")
        mode = RunnerConcurrencyMode(as_str(field_value(raw, "mode"), "mode"))
        if mode is RunnerConcurrencyMode.EXCLUSIVE:
            return cls.exclusive()
        if mode is RunnerConcurrencyMode.REENTRANT:
            return cls.reentrant(
                as_int(field_value(raw, "max_inflight_batches"), "max_inflight_batches"),
                as_int(field_value(raw, "max_inflight_entries"), "max_inflight_entries"),
            )
        return cls.sharded(as_int(field_value(raw, "instances"), "instances"))

    def to_json_value(self) -> JsonDict:
        if self.mode is RunnerConcurrencyMode.EXCLUSIVE:
            return {"mode": self.mode.value}
        if self.mode is RunnerConcurrencyMode.REENTRANT:
            if self.max_inflight_batches is None or self.max_inflight_entries is None:
                raise TypeError("reentrant concurrency requires batch and entry limits")
            return {
                "mode": self.mode.value,
                "max_inflight_batches": self.max_inflight_batches,
                "max_inflight_entries": self.max_inflight_entries,
            }
        if self.instances is None:
            raise TypeError("sharded concurrency requires instances")
        return {"mode": self.mode.value, "instances": self.instances}


class RunnerStatus(StrEnum):
    COMPLETED = "completed"
    WAITING = "waiting"
    BLOCKED = "blocked"
    CONTINUE = "continue"
    FAILED = "failed"
    CANCELLED = "cancelled"


class RunnerMode(StrEnum):
    SCALAR_ADAPTER = "scalar_adapter"
    NATIVE_BATCH = "native_batch"
    BATCH = "batch"


class RunnerSideEffect(StrEnum):
    NONE = "none"
    RESOURCE = "resource"
    EXTERNAL = "external"
    UNKNOWN = "unknown"


class TimeoutGranularity(StrEnum):
    BATCH = "batch"
    ENTRY = "entry"


@dataclass(frozen=True)
class RunnerBatchCapability:
    mode: RunnerMode = RunnerMode.SCALAR_ADAPTER
    preferred_batch_size: int = 1
    max_batch_entries: int = 1
    max_entry_concurrency: int = 1
    max_inflight_batches: int = 1
    scalar_thread_safe: bool = False
    scalar_reentrant: bool = False
    partial_failure: bool = True
    preserve_order: bool = True
    side_effect: RunnerSideEffect = RunnerSideEffect.UNKNOWN

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "RunnerBatchCapability")
        return cls(
            mode=RunnerMode(as_str(field_value(raw, "mode"), "mode")),
            preferred_batch_size=as_int(
                field_value(raw, "preferred_batch_size"), "preferred_batch_size"
            ),
            max_batch_entries=as_int(field_value(raw, "max_batch_entries"), "max_batch_entries"),
            max_entry_concurrency=as_int(
                field_value(raw, "max_entry_concurrency"), "max_entry_concurrency"
            ),
            max_inflight_batches=as_int(
                field_value(raw, "max_inflight_batches"), "max_inflight_batches"
            ),
            scalar_thread_safe=as_bool(
                field_value(raw, "scalar_thread_safe"), "scalar_thread_safe"
            ),
            scalar_reentrant=as_bool(field_value(raw, "scalar_reentrant"), "scalar_reentrant"),
            partial_failure=as_bool(field_value(raw, "partial_failure"), "partial_failure"),
            preserve_order=as_bool(field_value(raw, "preserve_order"), "preserve_order"),
            side_effect=RunnerSideEffect(as_str(field_value(raw, "side_effect"), "side_effect")),
        )


@dataclass(frozen=True)
class RunnerPayloadCapability:
    layouts: tuple[PayloadLayout, ...] = (PayloadLayout.ROW,)
    preferred_layout: PayloadLayout = PayloadLayout.ROW
    zero_copy: bool = False

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "RunnerPayloadCapability")
        return cls(
            layouts=tuple(
                PayloadLayout(as_str(item, "layouts"))
                for item in as_str_tuple(field_value(raw, "layouts"), "layouts")
            ),
            preferred_layout=PayloadLayout(
                as_str(field_value(raw, "preferred_layout"), "preferred_layout")
            ),
            zero_copy=as_bool(field_value(raw, "zero_copy"), "zero_copy"),
        )


@dataclass(frozen=True)
class RunnerResourceCapability:
    batch_read: bool = False
    batch_write: bool = False
    requires_resource_plan: bool = True
    supports_shared_memory: bool = False

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "RunnerResourceCapability")
        return cls(
            batch_read=as_bool(field_value(raw, "batch_read"), "batch_read"),
            batch_write=as_bool(field_value(raw, "batch_write"), "batch_write"),
            requires_resource_plan=as_bool(
                field_value(raw, "requires_resource_plan"), "requires_resource_plan"
            ),
            supports_shared_memory=as_bool(
                field_value(raw, "supports_shared_memory"), "supports_shared_memory"
            ),
        )


@dataclass(frozen=True)
class RunnerOrderingCapability:
    default: OrderingRequirement = field(default_factory=OrderingRequirement.none)
    supports_sequence: bool = True
    supports_same_resource_order: bool = True

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "RunnerOrderingCapability")
        return cls(
            default=OrderingRequirement.from_json_dict(
                as_mapping(field_value(raw, "default"), "default")
            ),
            supports_sequence=as_bool(field_value(raw, "supports_sequence"), "supports_sequence"),
            supports_same_resource_order=as_bool(
                field_value(raw, "supports_same_resource_order"),
                "supports_same_resource_order",
            ),
        )


@dataclass(frozen=True)
class RunnerControlCapability:
    entry_cancel: bool = False
    batch_cancel: bool = True
    timeout_granularity: TimeoutGranularity = TimeoutGranularity.BATCH

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "RunnerControlCapability")
        return cls(
            entry_cancel=as_bool(field_value(raw, "entry_cancel"), "entry_cancel"),
            batch_cancel=as_bool(field_value(raw, "batch_cancel"), "batch_cancel"),
            timeout_granularity=TimeoutGranularity(
                as_str(field_value(raw, "timeout_granularity"), "timeout_granularity")
            ),
        )


@dataclass(frozen=True)
class RunnerDescriptor:
    runner_id: str
    plugin_id: str
    plugin_generation: int
    accepted_protocol_ids: tuple[str, ...]
    purity: RunnerPurity
    execution_class: ExecutionClass
    invocation_mode: InvocationMode = InvocationMode.SYNC_EXCLUSIVE
    concurrency: RunnerConcurrency = field(default_factory=RunnerConcurrency.exclusive)
    input_schema: JsonDict = field(default_factory=dict)
    output_schema: JsonDict = field(default_factory=dict)
    batch: RunnerBatchCapability = field(default_factory=RunnerBatchCapability)
    payload: RunnerPayloadCapability = field(default_factory=RunnerPayloadCapability)
    resources: RunnerResourceCapability = field(default_factory=RunnerResourceCapability)
    ordering: RunnerOrderingCapability = field(default_factory=RunnerOrderingCapability)
    control: RunnerControlCapability = field(default_factory=RunnerControlCapability)
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
            invocation_mode=InvocationMode(
                as_str(raw.get("invocation_mode", "sync_exclusive"), "invocation_mode")
            ),
            concurrency=RunnerConcurrency.from_json_dict(
                as_mapping(raw.get("concurrency", {"mode": "exclusive"}), "concurrency")
            ),
            input_schema=as_json_dict(field_value(raw, "input_schema"), "input_schema"),
            output_schema=as_json_dict(field_value(raw, "output_schema"), "output_schema"),
            batch=RunnerBatchCapability.from_json_dict(
                as_mapping(field_value(raw, "batch"), "batch")
            ),
            payload=RunnerPayloadCapability.from_json_dict(
                as_mapping(field_value(raw, "payload"), "payload")
            ),
            resources=RunnerResourceCapability.from_json_dict(
                as_mapping(field_value(raw, "resources"), "resources")
            ),
            ordering=RunnerOrderingCapability.from_json_dict(
                as_mapping(field_value(raw, "ordering"), "ordering")
            ),
            control=RunnerControlCapability.from_json_dict(
                as_mapping(field_value(raw, "control"), "control")
            ),
            metadata=as_scalar_dict(field_value(raw, "metadata"), "metadata"),
            contract_surfaces=as_str_tuple(
                field_value(raw, "contract_surfaces"), "contract_surfaces"
            ),
        )


@dataclass(frozen=True)
class RunnerContext:
    registry_generation: int
    current_step: int
    tick_id: str
    batch_id: str
    executor_id: str
    task_lease_ids: tuple[str, ...]
    entry_count: int
    invocation_id: str = ""
    cancel_token: str = ""
    deadline_tick: int | None = None
    deadline_after_ms: int | None = None
    cancel_requested: bool = False

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "RunnerContext")
        return cls(
            registry_generation=as_int(
                field_value(raw, "registry_generation"), "registry_generation"
            ),
            current_step=as_int(field_value(raw, "current_step"), "current_step"),
            tick_id=as_str(field_value(raw, "tick_id"), "tick_id"),
            batch_id=as_str(field_value(raw, "batch_id"), "batch_id"),
            executor_id=as_str(field_value(raw, "executor_id"), "executor_id"),
            task_lease_ids=as_str_tuple(field_value(raw, "task_lease_ids"), "task_lease_ids"),
            entry_count=as_int(field_value(raw, "entry_count"), "entry_count"),
            invocation_id=as_str(field_value(raw, "invocation_id"), "invocation_id"),
            cancel_token=as_str(field_value(raw, "cancel_token"), "cancel_token"),
            deadline_tick=optional_int(field_value(raw, "deadline_tick"), "deadline_tick"),
            deadline_after_ms=optional_int(raw.get("deadline_after_ms"), "deadline_after_ms"),
            cancel_requested=as_bool(field_value(raw, "cancel_requested"), "cancel_requested"),
        )


@dataclass(frozen=True)
class RunnerResult:
    task_id: str
    output: JsonValue = None
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
            output=as_json_value(raw.get("output")),
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
