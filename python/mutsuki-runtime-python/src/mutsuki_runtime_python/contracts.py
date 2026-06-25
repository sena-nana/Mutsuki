from __future__ import annotations

import json
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field, fields, is_dataclass
from enum import StrEnum
from typing import Self, cast

ScalarValue = str | int | float | bool
JsonValue = None | bool | int | float | str | list["JsonValue"] | dict[str, "JsonValue"]
JsonDict = dict[str, JsonValue]

ERR_RUNTIME_HOST_FAILED = "runtime.host_failed"
ERR_CAPABILITY_EXHAUSTED = "capability.exhausted"
ERR_TASK_NOT_FOUND = "task.not_found"
ERR_RUNNER_NOT_FOUND = "runner.not_found"
ERR_REGISTRY_UNAUTHORIZED = "registry.unauthorized"
ERR_RESOURCE_NOT_FOUND = "resource.not_found"
ERR_RESOURCE_GENERATION_MISMATCH = "resource.generation_mismatch"
ERR_RESOURCE_LEASE_EXPIRED = "resource.lease_expired"
ERR_STATE_CONFLICT = "state.conflict"
ERR_RELOAD_BLOCKED = "plugin.reload_blocked"


class TaskStatus(StrEnum):
    PENDING = "pending"
    RUNNING = "running"
    COMPLETED = "completed"
    FAILED = "failed"
    CANCELLED = "cancelled"


class RunnerPurity(StrEnum):
    PURE = "pure"
    COMMITTER = "committer"
    EFFECTFUL = "effectful"


class RunnerStatus(StrEnum):
    COMPLETED = "completed"
    CONTINUE = "continue"
    FAILED = "failed"
    CANCELLED = "cancelled"


class ConflictPolicy(StrEnum):
    RETRY = "retry"
    MERGE = "merge"
    DISCARD = "discard"
    FAIL = "fail"
    EMIT_CONFLICT_TASK = "emit_conflict_task"


class RuntimeEventKind(StrEnum):
    TASK = "task"
    RUNNER = "runner"
    STATE = "state"
    EFFECT = "effect"
    PLUGIN = "plugin"
    RESOURCE = "resource"
    TRACE = "trace"
    RELOAD = "reload"
    HOST = "host"


class SpanStatus(StrEnum):
    OK = "ok"
    ERROR = "error"


class ResourceLifetime(StrEnum):
    BORROWED_UNTIL_TASK_END = "borrowed_until_task_end"
    LEASE_UNTIL = "lease_until"
    PERSISTENT = "persistent"
    EXTERNAL_MANAGED = "external_managed"


class ValueStorage(StrEnum):
    INLINE_SMALL = "inline_small"
    LOCAL_VALUE_STORE = "local_value_store"
    BLOB = "blob"
    STREAM = "stream"
    PROVIDER_RPC = "provider_rpc"


class ResourceSealState(StrEnum):
    WRITABLE = "writable"
    SEALED = "sealed"


class ContractSurfaceKind(StrEnum):
    RUNNER = "runner"
    TASK_KIND = "task_kind"
    SCHEMA = "schema"
    RESOURCE_SCHEMA = "resource_schema"
    RESOURCE_PROVIDER = "resource_provider"
    EFFECT = "effect"
    STREAM = "stream"
    SUBSCRIPTION = "subscription"
    TIMER = "timer"
    TASK_DEMAND = "task_demand"
    STATE_SCHEMA = "state_schema"
    LIFECYCLE = "lifecycle"
    PERMISSION = "permission"


class SurfaceCompatibility(StrEnum):
    IDENTICAL = "identical"
    ADDITIVE = "additive"
    DEPRECATED = "deprecated"
    REMOVED = "removed"
    BREAKING = "breaking"


class SurfaceOccupancyHandleKind(StrEnum):
    STREAM = "stream"
    SUBSCRIPTION = "subscription"
    TIMER = "timer"


def _as_mapping(data: object, contract: str) -> Mapping[str, object]:
    if not isinstance(data, Mapping):
        raise TypeError(f"{contract} expects a mapping")
    return data


def _field(data: Mapping[str, object], field_name: str) -> object:
    if field_name not in data:
        raise TypeError(f"{field_name} is required")
    return data[field_name]


def _as_str(value: object, field_name: str) -> str:
    if not isinstance(value, str):
        raise TypeError(f"{field_name} expects str")
    return value


def _as_int(value: object, field_name: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool):
        raise TypeError(f"{field_name} expects int")
    return value


def _as_bool(value: object, field_name: str) -> bool:
    if not isinstance(value, bool):
        raise TypeError(f"{field_name} expects bool")
    return value


def _as_scalar(value: object, field_name: str) -> ScalarValue:
    if isinstance(value, str | bool | int | float):
        return value
    raise TypeError(f"{field_name} expects scalar")


def _as_str_tuple(value: object, field_name: str) -> tuple[str, ...]:
    if not isinstance(value, Sequence) or isinstance(value, str | bytes | bytearray):
        raise TypeError(f"{field_name} expects sequence")
    return tuple(_as_str(item, field_name) for item in value)


def _as_json_value(value: object) -> JsonValue:
    if value is None or isinstance(value, bool | int | float | str):
        return value
    if isinstance(value, Mapping):
        return {str(key): _as_json_value(item) for key, item in value.items()}
    if isinstance(value, Sequence) and not isinstance(value, str | bytes | bytearray):
        return [_as_json_value(item) for item in value]
    raise TypeError(f"value is not JSON serializable: {type(value).__qualname__}")


def _as_json_dict(value: object, field_name: str) -> JsonDict:
    converted = _as_json_value(value)
    if not isinstance(converted, dict):
        raise TypeError(f"{field_name} expects mapping")
    return converted


def _as_scalar_dict(value: object, field_name: str) -> dict[str, ScalarValue]:
    if not isinstance(value, Mapping):
        raise TypeError(f"{field_name} expects mapping")
    return {str(key): _as_scalar(item, field_name) for key, item in value.items()}


def _to_json_value(value: object) -> JsonValue:
    if isinstance(value, StrEnum):
        return value.value
    if is_dataclass(value):
        return {field.name: _to_json_value(getattr(value, field.name)) for field in fields(value)}
    if isinstance(value, Mapping):
        return {str(key): _to_json_value(item) for key, item in value.items()}
    if isinstance(value, tuple | list):
        return [_to_json_value(item) for item in value]
    if value is None or isinstance(value, bool | int | float | str):
        return value
    raise TypeError(f"value is not JSON serializable: {type(value).__qualname__}")


def to_json_dict(value: object) -> JsonDict:
    converted = _to_json_value(value)
    if not isinstance(converted, dict):
        raise TypeError("top-level value must serialize to a JSON object")
    return converted


def to_json_bytes(value: object) -> bytes:
    return json.dumps(to_json_dict(value), separators=(",", ":"), ensure_ascii=False).encode()


def from_json_dict[T](contract_type: type[T], data: Mapping[str, object] | JsonDict) -> T:
    decoder = getattr(contract_type, "from_json_dict", None)
    if decoder is None:
        raise TypeError(f"{contract_type.__qualname__} does not expose from_json_dict")
    return cast(T, decoder(data))


def from_json_bytes[T](contract_type: type[T], data: bytes | bytearray | str) -> T:
    loaded = json.loads(data)
    if not isinstance(loaded, Mapping):
        raise TypeError("top-level JSON value must be an object")
    return from_json_dict(contract_type, loaded)


@dataclass(frozen=True)
class RuntimeError:
    code: str
    source: str
    route: str
    lost_capability: str | None = None
    recovery: str | None = None
    cause: RuntimeError | None = None
    evidence: dict[str, ScalarValue] = field(default_factory=dict)

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "RuntimeError")
        cause = _field(raw, "cause")
        lost_capability = _field(raw, "lost_capability")
        recovery = _field(raw, "recovery")
        return cls(
            code=_as_str(_field(raw, "code"), "code"),
            source=_as_str(_field(raw, "source"), "source"),
            route=_as_str(_field(raw, "route"), "route"),
            lost_capability=None
            if lost_capability is None
            else _as_str(lost_capability, "lost_capability"),
            recovery=None if recovery is None else _as_str(recovery, "recovery"),
            cause=None
            if cause is None
            else RuntimeError.from_json_dict(_as_mapping(cause, "cause")),
            evidence=_as_scalar_dict(_field(raw, "evidence"), "evidence"),
        )


@dataclass(frozen=True)
class VersionExpectation:
    ref_id: str
    expected_version: int

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "VersionExpectation")
        return cls(
            ref_id=_as_str(_field(raw, "ref_id"), "ref_id"),
            expected_version=_as_int(_field(raw, "expected_version"), "expected_version"),
        )


@dataclass(frozen=True)
class Task:
    task_id: str
    kind: str
    priority: int
    ready_at_step: int | None
    payload: JsonValue
    input_refs: tuple[str, ...]
    expected_versions: tuple[VersionExpectation, ...]
    correlation_id: str | None
    idempotency_key: str | None
    runner_hint: str | None
    registry_generation: int
    required_surfaces: tuple[str, ...]
    created_sequence: int

    @classmethod
    def new(cls, task_id: str, kind: str, payload: JsonValue = None) -> Self:
        return cls(
            task_id=task_id,
            kind=kind,
            priority=0,
            ready_at_step=None,
            payload=payload,
            input_refs=(),
            expected_versions=(),
            correlation_id=None,
            idempotency_key=None,
            runner_hint=None,
            registry_generation=0,
            required_surfaces=(),
            created_sequence=0,
        )

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "Task")
        raw_expected = _field(raw, "expected_versions")
        if not isinstance(raw_expected, Sequence) or isinstance(
            raw_expected, str | bytes | bytearray
        ):
            raise TypeError("expected_versions expects sequence")
        ready_at_step = _field(raw, "ready_at_step")
        correlation_id = _field(raw, "correlation_id")
        idempotency_key = _field(raw, "idempotency_key")
        runner_hint = _field(raw, "runner_hint")
        return cls(
            task_id=_as_str(_field(raw, "task_id"), "task_id"),
            kind=_as_str(_field(raw, "kind"), "kind"),
            priority=_as_int(_field(raw, "priority"), "priority"),
            ready_at_step=None
            if ready_at_step is None
            else _as_int(ready_at_step, "ready_at_step"),
            payload=_as_json_value(_field(raw, "payload")),
            input_refs=_as_str_tuple(_field(raw, "input_refs"), "input_refs"),
            expected_versions=tuple(
                VersionExpectation.from_json_dict(_as_mapping(item, "VersionExpectation"))
                for item in raw_expected
            ),
            correlation_id=None
            if correlation_id is None
            else _as_str(correlation_id, "correlation_id"),
            idempotency_key=None
            if idempotency_key is None
            else _as_str(idempotency_key, "idempotency_key"),
            runner_hint=None if runner_hint is None else _as_str(runner_hint, "runner_hint"),
            registry_generation=_as_int(_field(raw, "registry_generation"), "registry_generation"),
            required_surfaces=_as_str_tuple(_field(raw, "required_surfaces"), "required_surfaces"),
            created_sequence=_as_int(_field(raw, "created_sequence"), "created_sequence"),
        )


@dataclass(frozen=True)
class RunnerDescriptor:
    runner_id: str
    plugin_id: str
    plugin_generation: int
    accepted_task_kinds: tuple[str, ...]
    purity: RunnerPurity
    input_schema: JsonDict = field(default_factory=dict)
    output_schema: JsonDict = field(default_factory=dict)
    metadata: dict[str, ScalarValue] = field(default_factory=dict)
    contract_surfaces: tuple[str, ...] = ()

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "RunnerDescriptor")
        return cls(
            runner_id=_as_str(_field(raw, "runner_id"), "runner_id"),
            plugin_id=_as_str(_field(raw, "plugin_id"), "plugin_id"),
            plugin_generation=_as_int(_field(raw, "plugin_generation"), "plugin_generation"),
            accepted_task_kinds=_as_str_tuple(
                _field(raw, "accepted_task_kinds"), "accepted_task_kinds"
            ),
            purity=RunnerPurity(_as_str(_field(raw, "purity"), "purity")),
            input_schema=_as_json_dict(_field(raw, "input_schema"), "input_schema"),
            output_schema=_as_json_dict(_field(raw, "output_schema"), "output_schema"),
            metadata=_as_scalar_dict(_field(raw, "metadata"), "metadata"),
            contract_surfaces=_as_str_tuple(_field(raw, "contract_surfaces"), "contract_surfaces"),
        )


@dataclass(frozen=True)
class RunnerContext:
    registry_generation: int
    current_step: int

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "RunnerContext")
        return cls(
            registry_generation=_as_int(_field(raw, "registry_generation"), "registry_generation"),
            current_step=_as_int(_field(raw, "current_step"), "current_step"),
        )


@dataclass(frozen=True)
class StateDelta:
    target_ref: str
    expected_version: int
    patch: JsonValue
    conflict_policy: ConflictPolicy

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "StateDelta")
        return cls(
            target_ref=_as_str(_field(raw, "target_ref"), "target_ref"),
            expected_version=_as_int(_field(raw, "expected_version"), "expected_version"),
            patch=_as_json_value(_field(raw, "patch")),
            conflict_policy=ConflictPolicy(
                _as_str(_field(raw, "conflict_policy"), "conflict_policy")
            ),
        )


@dataclass(frozen=True)
class DomainEvent:
    event_id: str
    kind: str
    payload: JsonValue

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "DomainEvent")
        return cls(
            event_id=_as_str(_field(raw, "event_id"), "event_id"),
            kind=_as_str(_field(raw, "kind"), "kind"),
            payload=_as_json_value(_field(raw, "payload")),
        )


@dataclass(frozen=True)
class EffectPrecondition:
    ref_id: str
    expected_version: int


@dataclass(frozen=True)
class EffectRequest:
    effect_id: str
    kind: str
    payload: JsonValue
    preconditions: tuple[EffectPrecondition, ...] = ()
    idempotency_key: str | None = None


@dataclass(frozen=True)
class RunnerResult:
    task_id: str
    deltas: tuple[StateDelta, ...] = ()
    events: tuple[DomainEvent, ...] = ()
    tasks: tuple[Task, ...] = ()
    effects: tuple[EffectRequest, ...] = ()
    values: tuple[ValueRef, ...] = ()
    resources: tuple[ResourceRef, ...] = ()
    status: RunnerStatus = RunnerStatus.COMPLETED

    @classmethod
    def completed(cls, task_id: str) -> Self:
        return cls(task_id=task_id)

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "RunnerResult")
        return cls(
            task_id=_as_str(_field(raw, "task_id"), "task_id"),
            deltas=tuple(
                StateDelta.from_json_dict(_as_mapping(item, "StateDelta"))
                for item in _sequence(_field(raw, "deltas"), "deltas")
            ),
            events=tuple(
                DomainEvent.from_json_dict(_as_mapping(item, "DomainEvent"))
                for item in _sequence(_field(raw, "events"), "events")
            ),
            tasks=tuple(
                Task.from_json_dict(_as_mapping(item, "Task"))
                for item in _sequence(_field(raw, "tasks"), "tasks")
            ),
            effects=tuple(
                _effect_request_from_json(_as_mapping(item, "EffectRequest"))
                for item in _sequence(_field(raw, "effects"), "effects")
            ),
            values=tuple(
                _value_ref_from_json(_as_mapping(item, "ValueRef"))
                for item in _sequence(_field(raw, "values"), "values")
            ),
            resources=tuple(
                ResourceRef.from_json_dict(_as_mapping(item, "ResourceRef"))
                for item in _sequence(_field(raw, "resources"), "resources")
            ),
            status=RunnerStatus(_as_str(_field(raw, "status"), "status")),
        )


@dataclass(frozen=True)
class LeaseToken:
    token_id: str
    ref_id: str
    owner: str
    mode: str
    expires_at_step: int | None
    generation: int


@dataclass(frozen=True)
class ResourceAccess:
    type: str
    path: str | None = None
    offset: int | None = None
    len: int | None = None
    readonly: bool | None = None
    store_id: str | None = None
    key: str | None = None
    endpoint: str | None = None


@dataclass(frozen=True)
class ResourceRef:
    ref_id: str
    provider_id: str
    resource_kind: str
    schema: str
    version: int
    generation: int
    access: ResourceAccess
    size_hint: int | None
    content_hash: str | None
    lifetime: ResourceLifetime
    lease: LeaseToken | None
    seal_state: ResourceSealState

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "ResourceRef")
        return cls(
            ref_id=_as_str(_field(raw, "ref_id"), "ref_id"),
            provider_id=_as_str(_field(raw, "provider_id"), "provider_id"),
            resource_kind=_as_str(_field(raw, "resource_kind"), "resource_kind"),
            schema=_as_str(_field(raw, "schema"), "schema"),
            version=_as_int(_field(raw, "version"), "version"),
            generation=_as_int(_field(raw, "generation"), "generation"),
            access=_resource_access_from_json(_as_mapping(_field(raw, "access"), "access")),
            size_hint=_optional_int(_field(raw, "size_hint"), "size_hint"),
            content_hash=_optional_str(_field(raw, "content_hash"), "content_hash"),
            lifetime=ResourceLifetime(_as_str(_field(raw, "lifetime"), "lifetime")),
            lease=None
            if _field(raw, "lease") is None
            else _lease_from_json(_as_mapping(_field(raw, "lease"), "lease")),
            seal_state=ResourceSealState(_as_str(_field(raw, "seal_state"), "seal_state")),
        )


@dataclass(frozen=True)
class ValueRef:
    ref_id: str
    provider_id: str
    schema: str
    version: int
    generation: int
    size_hint: int | None
    content_hash: str | None
    lifetime: ResourceLifetime
    storage: ValueStorage


@dataclass(frozen=True)
class RuntimeEvent:
    sequence: int
    kind: RuntimeEventKind
    name: str
    subject_id: str | None
    attributes: dict[str, ScalarValue]
    error: RuntimeError | None


@dataclass(frozen=True)
class TraceSpan:
    trace_id: str
    span_id: str
    parent_span_id: str | None
    name: str
    start: float
    end: float | None
    attributes: dict[str, ScalarValue]
    status: SpanStatus


@dataclass(frozen=True)
class ContractSurface:
    surface_id: str
    kind: ContractSurfaceKind
    owner_plugin_id: str
    fingerprint: str
    deprecated: bool


@dataclass(frozen=True)
class SurfaceOccupancyHandle:
    handle_id: str
    surface_id: str
    owner_plugin_id: str
    plugin_generation: int
    registry_generation: int
    kind: SurfaceOccupancyHandleKind

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "SurfaceOccupancyHandle")
        return cls(
            handle_id=_as_str(_field(raw, "handle_id"), "handle_id"),
            surface_id=_as_str(_field(raw, "surface_id"), "surface_id"),
            owner_plugin_id=_as_str(_field(raw, "owner_plugin_id"), "owner_plugin_id"),
            plugin_generation=_as_int(_field(raw, "plugin_generation"), "plugin_generation"),
            registry_generation=_as_int(_field(raw, "registry_generation"), "registry_generation"),
            kind=SurfaceOccupancyHandleKind(_as_str(_field(raw, "kind"), "kind")),
        )


@dataclass(frozen=True)
class RuntimeLoadPlan:
    lock_version: int
    core_api_version: str
    profile_id: str
    profile_hash: str
    registry_generation: int
    plugins: tuple[JsonDict, ...]
    load_order: tuple[str, ...]
    runner_bindings: dict[str, str]
    contract_surfaces: tuple[ContractSurface, ...]


def _sequence(value: object, field_name: str) -> Sequence[object]:
    if not isinstance(value, Sequence) or isinstance(value, str | bytes | bytearray):
        raise TypeError(f"{field_name} expects sequence")
    return value


def _optional_str(value: object, field_name: str) -> str | None:
    return None if value is None else _as_str(value, field_name)


def _optional_int(value: object, field_name: str) -> int | None:
    return None if value is None else _as_int(value, field_name)


def _lease_from_json(raw: Mapping[str, object]) -> LeaseToken:
    return LeaseToken(
        token_id=_as_str(_field(raw, "token_id"), "token_id"),
        ref_id=_as_str(_field(raw, "ref_id"), "ref_id"),
        owner=_as_str(_field(raw, "owner"), "owner"),
        mode=_as_str(_field(raw, "mode"), "mode"),
        expires_at_step=_optional_int(_field(raw, "expires_at_step"), "expires_at_step"),
        generation=_as_int(_field(raw, "generation"), "generation"),
    )


def _resource_access_from_json(raw: Mapping[str, object]) -> ResourceAccess:
    return ResourceAccess(
        type=_as_str(_field(raw, "type"), "type"),
        path=_optional_str(raw.get("path"), "path"),
        offset=_optional_int(raw.get("offset"), "offset"),
        len=_optional_int(raw.get("len"), "len"),
        readonly=None if raw.get("readonly") is None else _as_bool(raw.get("readonly"), "readonly"),
        store_id=_optional_str(raw.get("store_id"), "store_id"),
        key=_optional_str(raw.get("key"), "key"),
        endpoint=_optional_str(raw.get("endpoint"), "endpoint"),
    )


def _effect_request_from_json(raw: Mapping[str, object]) -> EffectRequest:
    return EffectRequest(
        effect_id=_as_str(_field(raw, "effect_id"), "effect_id"),
        kind=_as_str(_field(raw, "kind"), "kind"),
        payload=_as_json_value(_field(raw, "payload")),
        preconditions=tuple(
            EffectPrecondition(
                ref_id=_as_str(_field(_as_mapping(item, "EffectPrecondition"), "ref_id"), "ref_id"),
                expected_version=_as_int(
                    _field(_as_mapping(item, "EffectPrecondition"), "expected_version"),
                    "expected_version",
                ),
            )
            for item in _sequence(_field(raw, "preconditions"), "preconditions")
        ),
        idempotency_key=_optional_str(_field(raw, "idempotency_key"), "idempotency_key"),
    )


def _value_ref_from_json(raw: Mapping[str, object]) -> ValueRef:
    return ValueRef(
        ref_id=_as_str(_field(raw, "ref_id"), "ref_id"),
        provider_id=_as_str(_field(raw, "provider_id"), "provider_id"),
        schema=_as_str(_field(raw, "schema"), "schema"),
        version=_as_int(_field(raw, "version"), "version"),
        generation=_as_int(_field(raw, "generation"), "generation"),
        size_hint=_optional_int(_field(raw, "size_hint"), "size_hint"),
        content_hash=_optional_str(_field(raw, "content_hash"), "content_hash"),
        lifetime=ResourceLifetime(_as_str(_field(raw, "lifetime"), "lifetime")),
        storage=ValueStorage(_as_str(_field(raw, "storage"), "storage")),
    )
