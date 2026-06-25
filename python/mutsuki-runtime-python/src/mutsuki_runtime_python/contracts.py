from __future__ import annotations

import json
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field, fields, is_dataclass
from enum import StrEnum
from typing import ClassVar, Self, cast

ScalarValue = str | int | float | bool
JsonValue = None | bool | int | float | str | list["JsonValue"] | dict[str, "JsonValue"]
JsonDict = dict[str, JsonValue]

ERR_PLUGIN_DISABLED = "plugin.disabled"
ERR_PLUGIN_NOT_FOUND = "plugin.not_found"
ERR_RUNTIME_HOST_FAILED = "runtime.host_failed"
ERR_RUNTIME_HOST_GENERATION_MISMATCH = "runtime.host_generation_mismatch"
ERR_CAPABILITY_EXHAUSTED = "capability.exhausted"
ERR_TASK_NOT_FOUND = "task.not_found"
ERR_TASK_CLAIM_CONFLICT = "task.claim_conflict"
ERR_RUNNER_NOT_FOUND = "runner.not_found"
ERR_RUNNER_PURITY_VIOLATION = "runner.purity_violation"
ERR_REGISTRY_FROZEN = "registry.frozen"
ERR_REGISTRY_UNAUTHORIZED = "registry.unauthorized"
ERR_REGISTRY_GENERATION_MISMATCH = "registry.generation_mismatch"
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
    LIFECYCLE = "lifecycle"
    PLUGIN = "plugin"
    RESOURCE = "resource"
    TRACE = "trace"
    HOST = "host"
    TASK = "task"
    RUNNER = "runner"
    STATE = "state"
    EFFECT = "effect"
    RELOAD = "reload"


class SpanStatus(StrEnum):
    OK = "ok"
    ERROR = "error"


class ValueStorage(StrEnum):
    INLINE_SMALL = "inline_small"
    LOCAL_VALUE_STORE = "local_value_store"
    BLOB = "blob"
    STREAM = "stream"
    PROVIDER_RPC = "provider_rpc"


class ResourceSealState(StrEnum):
    WRITABLE = "writable"
    SEALED = "sealed"


class ArtifactType(StrEnum):
    ABI = "abi"
    PROCESS = "process"
    WASM = "wasm"
    PYTHON = "python"
    NATIVE = "native"


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


@dataclass(frozen=True)
class ResourceLifetime:
    kind: str
    lease_until_step: int | None = None

    BORROWED_UNTIL_TASK_END: ClassVar[ResourceLifetime]
    PERSISTENT: ClassVar[ResourceLifetime]
    EXTERNAL_MANAGED: ClassVar[ResourceLifetime]

    @classmethod
    def lease_until(cls, step: int) -> Self:
        return cls("lease_until", step)


ResourceLifetime.BORROWED_UNTIL_TASK_END = ResourceLifetime("borrowed_until_task_end")
ResourceLifetime.PERSISTENT = ResourceLifetime("persistent")
ResourceLifetime.EXTERNAL_MANAGED = ResourceLifetime("external_managed")


@dataclass(frozen=True)
class ResourceAccess:
    type: str
    path: str | None = None
    name: str | None = None
    offset: int | None = None
    len: int | None = None
    readonly: bool | None = None
    store_id: str | None = None
    key: str | None = None
    endpoint: str | None = None
    provider_id: str | None = None
    method: str | None = None

    @classmethod
    def inline(cls) -> Self:
        return cls(type="inline")

    @classmethod
    def mmap_file(cls, path: str, offset: int, len: int, readonly: bool) -> Self:
        return cls(type="mmap_file", path=path, offset=offset, len=len, readonly=readonly)

    @classmethod
    def shared_memory(cls, name: str, offset: int, len: int, readonly: bool) -> Self:
        return cls(type="shared_memory", name=name, offset=offset, len=len, readonly=readonly)

    @classmethod
    def blob(cls, store_id: str, key: str) -> Self:
        return cls(type="blob", store_id=store_id, key=key)

    @classmethod
    def stream(cls, endpoint: str) -> Self:
        return cls(type="stream", endpoint=endpoint)

    @classmethod
    def provider_rpc(cls, provider_id: str, method: str) -> Self:
        return cls(type="provider_rpc", provider_id=provider_id, method=method)

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        return cast(Self, _resource_access_from_json(_as_mapping(data, "ResourceAccess")))


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


def _sequence(value: object, field_name: str) -> Sequence[object]:
    if not isinstance(value, Sequence) or isinstance(value, str | bytes | bytearray):
        raise TypeError(f"{field_name} expects sequence")
    return value


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


def _optional_str(value: object, field_name: str) -> str | None:
    return None if value is None else _as_str(value, field_name)


def _optional_int(value: object, field_name: str) -> int | None:
    return None if value is None else _as_int(value, field_name)


def _to_json_value(value: object) -> JsonValue:
    if isinstance(value, StrEnum):
        return value.value
    if isinstance(value, ResourceLifetime):
        return _resource_lifetime_to_json(value)
    if isinstance(value, ResourceAccess):
        return _resource_access_to_json(value)
    if isinstance(value, TaskMatchRule):
        return _task_match_rule_to_json(value)
    if isinstance(value, ResourceValue):
        return _resource_value_to_json(value)
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


def _tuple_from_json[T](
    raw: Mapping[str, object], field_name: str, contract_type: type[T]
) -> tuple[T, ...]:
    return tuple(
        from_json_dict(contract_type, _as_mapping(item, contract_type.__qualname__))
        for item in _sequence(_field(raw, field_name), field_name)
    )


def _as_str_dict(raw: Mapping[str, object], field_name: str) -> dict[str, str]:
    value = _field(raw, field_name)
    if not isinstance(value, Mapping):
        raise TypeError(f"{field_name} expects mapping")
    return {str(key): _as_str(item, field_name) for key, item in value.items()}


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
            expected_versions=_tuple_from_json(raw, "expected_versions", VersionExpectation),
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
class StateRef:
    ref_id: str
    schema: str
    version: int

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "StateRef")
        return cls(
            ref_id=_as_str(_field(raw, "ref_id"), "ref_id"),
            schema=_as_str(_field(raw, "schema"), "schema"),
            version=_as_int(_field(raw, "version"), "version"),
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
class TaskMatchRule:
    type: str
    kind: str | None = None
    prefix: str | None = None

    @classmethod
    def kind_rule(cls, kind: str) -> Self:
        return cls(type="kind", kind=kind)

    @classmethod
    def kind_prefix(cls, prefix: str) -> Self:
        return cls(type="kind_prefix", prefix=prefix)

    @classmethod
    def any(cls) -> Self:
        return cls(type="any")

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        return cast(Self, _task_match_rule_from_json(_as_mapping(data, "TaskMatchRule")))


@dataclass(frozen=True)
class TaskDemand:
    demand_id: str
    plugin_id: str
    match_rule: TaskMatchRule
    target_task_kind: str
    target_runner_hint: str | None
    priority: int
    payload_projection: JsonValue
    input_ref_policy: str

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "TaskDemand")
        target_runner_hint = _field(raw, "target_runner_hint")
        return cls(
            demand_id=_as_str(_field(raw, "demand_id"), "demand_id"),
            plugin_id=_as_str(_field(raw, "plugin_id"), "plugin_id"),
            match_rule=TaskMatchRule.from_json_dict(
                _as_mapping(_field(raw, "match_rule"), "match_rule")
            ),
            target_task_kind=_as_str(_field(raw, "target_task_kind"), "target_task_kind"),
            target_runner_hint=None
            if target_runner_hint is None
            else _as_str(target_runner_hint, "target_runner_hint"),
            priority=_as_int(_field(raw, "priority"), "priority"),
            payload_projection=_as_json_value(_field(raw, "payload_projection")),
            input_ref_policy=_as_str(_field(raw, "input_ref_policy"), "input_ref_policy"),
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

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "EffectPrecondition")
        return cls(
            ref_id=_as_str(_field(raw, "ref_id"), "ref_id"),
            expected_version=_as_int(_field(raw, "expected_version"), "expected_version"),
        )


@dataclass(frozen=True)
class EffectRequest:
    effect_id: str
    kind: str
    payload: JsonValue
    preconditions: tuple[EffectPrecondition, ...] = ()
    idempotency_key: str | None = None

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "EffectRequest")
        return cls(
            effect_id=_as_str(_field(raw, "effect_id"), "effect_id"),
            kind=_as_str(_field(raw, "kind"), "kind"),
            payload=_as_json_value(_field(raw, "payload")),
            preconditions=_tuple_from_json(raw, "preconditions", EffectPrecondition),
            idempotency_key=_optional_str(_field(raw, "idempotency_key"), "idempotency_key"),
        )


@dataclass(frozen=True)
class LeaseToken:
    token_id: str
    ref_id: str
    owner: str
    mode: str
    expires_at_step: int | None
    generation: int

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "LeaseToken")
        return cls(
            token_id=_as_str(_field(raw, "token_id"), "token_id"),
            ref_id=_as_str(_field(raw, "ref_id"), "ref_id"),
            owner=_as_str(_field(raw, "owner"), "owner"),
            mode=_as_str(_field(raw, "mode"), "mode"),
            expires_at_step=_optional_int(_field(raw, "expires_at_step"), "expires_at_step"),
            generation=_as_int(_field(raw, "generation"), "generation"),
        )


@dataclass(frozen=True)
class ExclusiveWriteLease:
    token: LeaseToken

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "ExclusiveWriteLease")
        return cls(token=LeaseToken.from_json_dict(_as_mapping(_field(raw, "token"), "token")))


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
            access=ResourceAccess.from_json_dict(_as_mapping(_field(raw, "access"), "access")),
            size_hint=_optional_int(_field(raw, "size_hint"), "size_hint"),
            content_hash=_optional_str(_field(raw, "content_hash"), "content_hash"),
            lifetime=_resource_lifetime_from_json(_field(raw, "lifetime")),
            lease=None
            if _field(raw, "lease") is None
            else LeaseToken.from_json_dict(_as_mapping(_field(raw, "lease"), "lease")),
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

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "ValueRef")
        return cls(
            ref_id=_as_str(_field(raw, "ref_id"), "ref_id"),
            provider_id=_as_str(_field(raw, "provider_id"), "provider_id"),
            schema=_as_str(_field(raw, "schema"), "schema"),
            version=_as_int(_field(raw, "version"), "version"),
            generation=_as_int(_field(raw, "generation"), "generation"),
            size_hint=_optional_int(_field(raw, "size_hint"), "size_hint"),
            content_hash=_optional_str(_field(raw, "content_hash"), "content_hash"),
            lifetime=_resource_lifetime_from_json(_field(raw, "lifetime")),
            storage=ValueStorage(_as_str(_field(raw, "storage"), "storage")),
        )


@dataclass(frozen=True)
class ResourceValue:
    type: str
    schema: str | None = None
    value: JsonValue = None
    version: int | None = None
    value_ref: ValueRef | None = None
    resource_ref: ResourceRef | None = None

    @classmethod
    def inline(cls, schema: str, value: JsonValue, version: int) -> Self:
        return cls(type="inline", schema=schema, value=value, version=version)

    @classmethod
    def value_ref_value(cls, value_ref: ValueRef) -> Self:
        return cls(type="value_ref", value_ref=value_ref)

    @classmethod
    def resource_ref_value(cls, resource_ref: ResourceRef) -> Self:
        return cls(type="resource_ref", resource_ref=resource_ref)

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        return cast(Self, _resource_value_from_json(_as_mapping(data, "ResourceValue")))


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
            deltas=_tuple_from_json(raw, "deltas", StateDelta),
            events=_tuple_from_json(raw, "events", DomainEvent),
            tasks=_tuple_from_json(raw, "tasks", Task),
            effects=_tuple_from_json(raw, "effects", EffectRequest),
            values=_tuple_from_json(raw, "values", ValueRef),
            resources=_tuple_from_json(raw, "resources", ResourceRef),
            status=RunnerStatus(_as_str(_field(raw, "status"), "status")),
        )


@dataclass(frozen=True)
class RuntimeEvent:
    sequence: int
    kind: RuntimeEventKind
    name: str
    subject_id: str | None
    attributes: dict[str, ScalarValue]
    error: RuntimeError | None

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "RuntimeEvent")
        return cls(
            sequence=_as_int(_field(raw, "sequence"), "sequence"),
            kind=RuntimeEventKind(_as_str(_field(raw, "kind"), "kind")),
            name=_as_str(_field(raw, "name"), "name"),
            subject_id=_optional_str(_field(raw, "subject_id"), "subject_id"),
            attributes=_as_scalar_dict(_field(raw, "attributes"), "attributes"),
            error=None
            if _field(raw, "error") is None
            else RuntimeError.from_json_dict(_as_mapping(_field(raw, "error"), "error")),
        )


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

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "TraceSpan")
        start = _field(raw, "start")
        end = _field(raw, "end")
        return cls(
            trace_id=_as_str(_field(raw, "trace_id"), "trace_id"),
            span_id=_as_str(_field(raw, "span_id"), "span_id"),
            parent_span_id=_optional_str(_field(raw, "parent_span_id"), "parent_span_id"),
            name=_as_str(_field(raw, "name"), "name"),
            start=float(_as_scalar(start, "start")),
            end=None if end is None else float(_as_scalar(end, "end")),
            attributes=_as_scalar_dict(_field(raw, "attributes"), "attributes"),
            status=SpanStatus(_as_str(_field(raw, "status"), "status")),
        )


@dataclass(frozen=True)
class PluginArtifact:
    artifact_type: ArtifactType
    path: str
    sha256: str

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "PluginArtifact")
        return cls(
            artifact_type=ArtifactType(_as_str(_field(raw, "artifact_type"), "artifact_type")),
            path=_as_str(_field(raw, "path"), "path"),
            sha256=_as_str(_field(raw, "sha256"), "sha256"),
        )


@dataclass(frozen=True)
class PermissionGrant:
    effects: tuple[str, ...]
    resources: tuple[str, ...]

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "PermissionGrant")
        return cls(
            effects=_as_str_tuple(_field(raw, "effects"), "effects"),
            resources=_as_str_tuple(_field(raw, "resources"), "resources"),
        )


@dataclass(frozen=True)
class LifecyclePolicy:
    reload_policy: str
    unload_timeout_ms: int
    supports_cancel: bool
    supports_dispose: bool
    supports_snapshot: bool

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "LifecyclePolicy")
        return cls(
            reload_policy=_as_str(_field(raw, "reload_policy"), "reload_policy"),
            unload_timeout_ms=_as_int(_field(raw, "unload_timeout_ms"), "unload_timeout_ms"),
            supports_cancel=_as_bool(_field(raw, "supports_cancel"), "supports_cancel"),
            supports_dispose=_as_bool(_field(raw, "supports_dispose"), "supports_dispose"),
            supports_snapshot=_as_bool(_field(raw, "supports_snapshot"), "supports_snapshot"),
        )


@dataclass(frozen=True)
class PluginProvides:
    runners: tuple[RunnerDescriptor, ...]
    task_demands: tuple[TaskDemand, ...]
    resource_schemas: tuple[str, ...]
    resource_providers: tuple[str, ...]
    effects: tuple[str, ...]
    streams: tuple[str, ...]
    subscriptions: tuple[str, ...]
    timers: tuple[str, ...]
    state_schemas: tuple[str, ...]

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "PluginProvides")
        return cls(
            runners=_tuple_from_json(raw, "runners", RunnerDescriptor),
            task_demands=_tuple_from_json(raw, "task_demands", TaskDemand),
            resource_schemas=_as_str_tuple(_field(raw, "resource_schemas"), "resource_schemas"),
            resource_providers=_as_str_tuple(
                _field(raw, "resource_providers"), "resource_providers"
            ),
            effects=_as_str_tuple(_field(raw, "effects"), "effects"),
            streams=_as_str_tuple(_field(raw, "streams"), "streams"),
            subscriptions=_as_str_tuple(_field(raw, "subscriptions"), "subscriptions"),
            timers=_as_str_tuple(_field(raw, "timers"), "timers"),
            state_schemas=_as_str_tuple(_field(raw, "state_schemas"), "state_schemas"),
        )


@dataclass(frozen=True)
class PluginManifest:
    plugin_id: str
    version: str
    api_version: str
    artifact: PluginArtifact
    provides: PluginProvides
    requires: tuple[str, ...]
    permissions: PermissionGrant
    lifecycle: LifecyclePolicy
    metadata: dict[str, ScalarValue]

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "PluginManifest")
        return cls(
            plugin_id=_as_str(_field(raw, "plugin_id"), "plugin_id"),
            version=_as_str(_field(raw, "version"), "version"),
            api_version=_as_str(_field(raw, "api_version"), "api_version"),
            artifact=PluginArtifact.from_json_dict(
                _as_mapping(_field(raw, "artifact"), "artifact")
            ),
            provides=PluginProvides.from_json_dict(
                _as_mapping(_field(raw, "provides"), "provides")
            ),
            requires=_as_str_tuple(_field(raw, "requires"), "requires"),
            permissions=PermissionGrant.from_json_dict(
                _as_mapping(_field(raw, "permissions"), "permissions")
            ),
            lifecycle=LifecyclePolicy.from_json_dict(
                _as_mapping(_field(raw, "lifecycle"), "lifecycle")
            ),
            metadata=_as_scalar_dict(_field(raw, "metadata"), "metadata"),
        )


@dataclass(frozen=True)
class RuntimeProfile:
    profile_id: str
    enabled_plugins: tuple[str, ...]
    bindings: dict[str, str]
    allow_dynamic_registration: bool
    allow_hot_reload: bool

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "RuntimeProfile")
        return cls(
            profile_id=_as_str(_field(raw, "profile_id"), "profile_id"),
            enabled_plugins=_as_str_tuple(_field(raw, "enabled_plugins"), "enabled_plugins"),
            bindings=_as_str_dict(raw, "bindings"),
            allow_dynamic_registration=_as_bool(
                _field(raw, "allow_dynamic_registration"), "allow_dynamic_registration"
            ),
            allow_hot_reload=_as_bool(_field(raw, "allow_hot_reload"), "allow_hot_reload"),
        )


@dataclass(frozen=True)
class ContractSurface:
    surface_id: str
    kind: ContractSurfaceKind
    owner_plugin_id: str
    fingerprint: str
    deprecated: bool

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "ContractSurface")
        return cls(
            surface_id=_as_str(_field(raw, "surface_id"), "surface_id"),
            kind=ContractSurfaceKind(_as_str(_field(raw, "kind"), "kind")),
            owner_plugin_id=_as_str(_field(raw, "owner_plugin_id"), "owner_plugin_id"),
            fingerprint=_as_str(_field(raw, "fingerprint"), "fingerprint"),
            deprecated=_as_bool(_field(raw, "deprecated"), "deprecated"),
        )


@dataclass(frozen=True)
class SurfaceOccupancy:
    surface_id: str
    pending_tasks: int
    running_invocations: int
    resource_refs: int
    state_refs: int
    active_leases: int
    open_streams: int
    subscriptions: int
    timers: int
    effect_inflight: int

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "SurfaceOccupancy")
        return cls(
            surface_id=_as_str(_field(raw, "surface_id"), "surface_id"),
            pending_tasks=_as_int(_field(raw, "pending_tasks"), "pending_tasks"),
            running_invocations=_as_int(
                _field(raw, "running_invocations"), "running_invocations"
            ),
            resource_refs=_as_int(_field(raw, "resource_refs"), "resource_refs"),
            state_refs=_as_int(_field(raw, "state_refs"), "state_refs"),
            active_leases=_as_int(_field(raw, "active_leases"), "active_leases"),
            open_streams=_as_int(_field(raw, "open_streams"), "open_streams"),
            subscriptions=_as_int(_field(raw, "subscriptions"), "subscriptions"),
            timers=_as_int(_field(raw, "timers"), "timers"),
            effect_inflight=_as_int(_field(raw, "effect_inflight"), "effect_inflight"),
        )

    def is_zero(self) -> bool:
        return (
            self.pending_tasks == 0
            and self.running_invocations == 0
            and self.resource_refs == 0
            and self.state_refs == 0
            and self.active_leases == 0
            and self.open_streams == 0
            and self.subscriptions == 0
            and self.timers == 0
            and self.effect_inflight == 0
        )


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
    plugins: tuple[PluginManifest, ...]
    load_order: tuple[str, ...]
    runner_bindings: dict[str, str]
    contract_surfaces: tuple[ContractSurface, ...]

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "RuntimeLoadPlan")
        return cls(
            lock_version=_as_int(_field(raw, "lock_version"), "lock_version"),
            core_api_version=_as_str(_field(raw, "core_api_version"), "core_api_version"),
            profile_id=_as_str(_field(raw, "profile_id"), "profile_id"),
            profile_hash=_as_str(_field(raw, "profile_hash"), "profile_hash"),
            registry_generation=_as_int(_field(raw, "registry_generation"), "registry_generation"),
            plugins=_tuple_from_json(raw, "plugins", PluginManifest),
            load_order=_as_str_tuple(_field(raw, "load_order"), "load_order"),
            runner_bindings=_as_str_dict(raw, "runner_bindings"),
            contract_surfaces=_tuple_from_json(raw, "contract_surfaces", ContractSurface),
        )


RuntimeLock = RuntimeLoadPlan


def _resource_lifetime_to_json(value: ResourceLifetime) -> JsonValue:
    if value.kind == "lease_until":
        if value.lease_until_step is None:
            raise TypeError("lease_until lifetime requires lease_until_step")
        return {"lease_until": value.lease_until_step}
    if value.lease_until_step is not None:
        raise TypeError("unit lifetime cannot carry lease_until_step")
    return value.kind


def _resource_lifetime_from_json(value: object) -> ResourceLifetime:
    if isinstance(value, str):
        if value == "borrowed_until_task_end":
            return ResourceLifetime.BORROWED_UNTIL_TASK_END
        if value == "persistent":
            return ResourceLifetime.PERSISTENT
        if value == "external_managed":
            return ResourceLifetime.EXTERNAL_MANAGED
        raise ValueError(f"unknown resource lifetime: {value}")
    raw = _as_mapping(value, "ResourceLifetime")
    if set(raw.keys()) != {"lease_until"}:
        raise TypeError("ResourceLifetime expects a unit string or {'lease_until': step}")
    return ResourceLifetime.lease_until(_as_int(raw["lease_until"], "lease_until"))


def _resource_access_to_json(value: ResourceAccess) -> JsonDict:
    if value.type == "inline":
        return {"type": "inline"}
    if value.type == "mmap_file":
        return {
            "type": "mmap_file",
            "path": _required_optional(value.path, "path"),
            "offset": _required_optional(value.offset, "offset"),
            "len": _required_optional(value.len, "len"),
            "readonly": _required_optional(value.readonly, "readonly"),
        }
    if value.type == "shared_memory":
        return {
            "type": "shared_memory",
            "name": _required_optional(value.name, "name"),
            "offset": _required_optional(value.offset, "offset"),
            "len": _required_optional(value.len, "len"),
            "readonly": _required_optional(value.readonly, "readonly"),
        }
    if value.type == "blob":
        return {
            "type": "blob",
            "store_id": _required_optional(value.store_id, "store_id"),
            "key": _required_optional(value.key, "key"),
        }
    if value.type == "stream":
        return {"type": "stream", "endpoint": _required_optional(value.endpoint, "endpoint")}
    if value.type == "provider_rpc":
        return {
            "type": "provider_rpc",
            "provider_id": _required_optional(value.provider_id, "provider_id"),
            "method": _required_optional(value.method, "method"),
        }
    raise ValueError(f"unknown resource access type: {value.type}")


def _resource_access_from_json(raw: Mapping[str, object]) -> ResourceAccess:
    access_type = _as_str(_field(raw, "type"), "type")
    if access_type == "inline":
        return ResourceAccess.inline()
    if access_type == "mmap_file":
        return ResourceAccess.mmap_file(
            path=_as_str(_field(raw, "path"), "path"),
            offset=_as_int(_field(raw, "offset"), "offset"),
            len=_as_int(_field(raw, "len"), "len"),
            readonly=_as_bool(_field(raw, "readonly"), "readonly"),
        )
    if access_type == "shared_memory":
        return ResourceAccess.shared_memory(
            name=_as_str(_field(raw, "name"), "name"),
            offset=_as_int(_field(raw, "offset"), "offset"),
            len=_as_int(_field(raw, "len"), "len"),
            readonly=_as_bool(_field(raw, "readonly"), "readonly"),
        )
    if access_type == "blob":
        return ResourceAccess.blob(
            store_id=_as_str(_field(raw, "store_id"), "store_id"),
            key=_as_str(_field(raw, "key"), "key"),
        )
    if access_type == "stream":
        return ResourceAccess.stream(endpoint=_as_str(_field(raw, "endpoint"), "endpoint"))
    if access_type == "provider_rpc":
        return ResourceAccess.provider_rpc(
            provider_id=_as_str(_field(raw, "provider_id"), "provider_id"),
            method=_as_str(_field(raw, "method"), "method"),
        )
    raise ValueError(f"unknown resource access type: {access_type}")


def _task_match_rule_to_json(value: TaskMatchRule) -> JsonDict:
    if value.type == "kind":
        return {"type": "kind", "kind": _required_optional(value.kind, "kind")}
    if value.type == "kind_prefix":
        return {"type": "kind_prefix", "prefix": _required_optional(value.prefix, "prefix")}
    if value.type == "any":
        return {"type": "any"}
    raise ValueError(f"unknown task match rule type: {value.type}")


def _task_match_rule_from_json(raw: Mapping[str, object]) -> TaskMatchRule:
    rule_type = _as_str(_field(raw, "type"), "type")
    if rule_type == "kind":
        return TaskMatchRule.kind_rule(_as_str(_field(raw, "kind"), "kind"))
    if rule_type == "kind_prefix":
        return TaskMatchRule.kind_prefix(_as_str(_field(raw, "prefix"), "prefix"))
    if rule_type == "any":
        return TaskMatchRule.any()
    raise ValueError(f"unknown task match rule type: {rule_type}")


def _resource_value_to_json(value: ResourceValue) -> JsonDict:
    if value.type == "inline":
        return {
            "type": "inline",
            "schema": _required_optional(value.schema, "schema"),
            "value": _to_json_value(value.value),
            "version": _required_optional(value.version, "version"),
        }
    if value.type == "value_ref":
        value_ref = _required_optional(value.value_ref, "value_ref")
        return {"type": "value_ref", **to_json_dict(value_ref)}
    if value.type == "resource_ref":
        resource_ref = _required_optional(value.resource_ref, "resource_ref")
        return {"type": "resource_ref", **to_json_dict(resource_ref)}
    raise ValueError(f"unknown resource value type: {value.type}")


def _resource_value_from_json(raw: Mapping[str, object]) -> ResourceValue:
    value_type = _as_str(_field(raw, "type"), "type")
    if value_type == "inline":
        return ResourceValue.inline(
            schema=_as_str(_field(raw, "schema"), "schema"),
            value=_as_json_value(_field(raw, "value")),
            version=_as_int(_field(raw, "version"), "version"),
        )
    if value_type == "value_ref":
        return ResourceValue.value_ref_value(ValueRef.from_json_dict(raw))
    if value_type == "resource_ref":
        return ResourceValue.resource_ref_value(ResourceRef.from_json_dict(raw))
    raise ValueError(f"unknown resource value type: {value_type}")


def _required_optional[T](value: T | None, field_name: str) -> T:
    if value is None:
        raise TypeError(f"{field_name} is required for this variant")
    return value
