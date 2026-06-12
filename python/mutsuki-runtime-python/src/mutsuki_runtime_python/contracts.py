from __future__ import annotations

import json
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field, fields, is_dataclass
from enum import StrEnum
from typing import Self, cast

ScalarValue = str | int | float | bool
JsonValue = None | bool | int | float | str | list["JsonValue"] | dict[str, "JsonValue"]
JsonDict = dict[str, JsonValue]

ERR_AGENT_NOT_FOUND = "agent.not_found"
ERR_CAPABILITY_EXHAUSTED = "capability.exhausted"
ERR_OPERATION_NOT_FOUND = "operation.not_found"
ERR_REF_NOT_FOUND = "ref.not_found"
ERR_RUNTIME_BACKEND_FAILED = "runtime.backend_failed"
ERR_RUNTIME_BACKEND_GENERATION_MISMATCH = "runtime.backend_generation_mismatch"
ERR_SCOPE_NO_MATCH = "scope.no_match"
ERR_SOURCE_UNREGISTERED = "source.unregistered"

class AgentPhase(StrEnum):
    SPAWN = "spawn"
    AWAKE = "awake"
    SLEEP = "sleep"
    STOP = "stop"


class AgentParticipation(StrEnum):
    PRIMARY_CANDIDATE = "primary_candidate"
    OBSERVER = "observer"
    EXPLICIT_HELPER = "explicit_helper"


class SideEffectPolicy(StrEnum):
    READ_ONLY = "read_only"
    ALLOW_EXTERNAL = "allow_external"


class OperationStatus(StrEnum):
    ACTIVE = "active"
    UNHEALTHY = "unhealthy"
    UNREGISTERING = "unregistering"
    NOT_FOUND = "not_found"


class PluginStatus(StrEnum):
    ENABLED = "enabled"
    DISABLED = "disabled"
    UNHEALTHY = "unhealthy"


class StrategyResultStatus(StrEnum):
    CONTINUE = "continue"
    WAIT_INPUT = "wait_input"
    COMPLETED = "completed"
    FAILED = "failed"


class SpanStatus(StrEnum):
    OK = "ok"
    ERROR = "error"


class RuntimeEventKind(StrEnum):
    LIFECYCLE = "lifecycle"
    ROUTING = "routing"
    OPERATION = "operation"
    RESOURCE = "resource"
    TRACE = "trace"
    BACKEND = "backend"


def _as_mapping(data: object, contract: str) -> Mapping[str, object]:
    if not isinstance(data, Mapping):
        raise TypeError(f"{contract} expects a mapping")
    return data


def _field(data: Mapping[str, object], field_name: str) -> object:
    if field_name not in data:
        raise TypeError(f"{field_name} is required")
    return data[field_name]


def _as_str(value: object, field_name: str) -> str:
    if value is None:
        raise TypeError(f"{field_name} expects str")
    if not isinstance(value, str):
        raise TypeError(f"{field_name} expects str")
    return value


def _as_int(value: object, field_name: str) -> int:
    if value is None:
        raise TypeError(f"{field_name} expects int")
    if not isinstance(value, int) or isinstance(value, bool):
        raise TypeError(f"{field_name} expects int")
    return value


def _as_float(value: object, field_name: str) -> float:
    if value is None:
        raise TypeError(f"{field_name} expects number")
    if not isinstance(value, int | float) or isinstance(value, bool):
        raise TypeError(f"{field_name} expects number")
    return float(value)


def _as_bool(value: object, field_name: str) -> bool:
    if value is None:
        raise TypeError(f"{field_name} expects bool")
    if not isinstance(value, bool):
        raise TypeError(f"{field_name} expects bool")
    return value


def _as_scalar(value: object, field_name: str) -> ScalarValue:
    if isinstance(value, str | bool | int | float):
        return value
    raise TypeError(f"{field_name} expects a scalar value")


def _as_str_tuple(value: object, field_name: str) -> tuple[str, ...]:
    if value is None:
        raise TypeError(f"{field_name} expects a sequence")
    if not isinstance(value, Sequence) or isinstance(value, str | bytes | bytearray):
        raise TypeError(f"{field_name} expects a sequence")
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
    if value is None:
        raise TypeError(f"{field_name} expects a mapping")
    if not isinstance(value, Mapping):
        raise TypeError(f"{field_name} expects a mapping")
    converted = _as_json_value(value)
    if not isinstance(converted, dict):
        raise TypeError(f"{field_name} expects a mapping")
    return converted


def _as_scalar_dict(value: object, field_name: str) -> dict[str, ScalarValue]:
    if value is None:
        raise TypeError(f"{field_name} expects a mapping")
    if not isinstance(value, Mapping):
        raise TypeError(f"{field_name} expects a mapping")
    return {str(key): _as_scalar(item, field_name) for key, item in value.items()}


def _to_json_value(value: object) -> JsonValue:
    if isinstance(value, StrEnum):
        return value.value
    if isinstance(value, ScopeRuleSpec):
        return value.to_json_dict()
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
class ScopeRuleSpec:
    type: str
    parts: tuple[ScopeRuleSpec, ...] = ()
    schema_id: str = ""
    prefix: str = ""
    source_id: str = ""
    kind: str = ""
    capability: str = ""
    field: str = ""
    value: ScalarValue | None = None

    @classmethod
    def always(cls) -> Self:
        return cls(type="always")

    @classmethod
    def never(cls) -> Self:
        return cls(type="never")

    @classmethod
    def all(cls, parts: Sequence[ScopeRuleSpec]) -> Self:
        return cls(type="all", parts=tuple(parts))

    @classmethod
    def any(cls, parts: Sequence[ScopeRuleSpec]) -> Self:
        return cls(type="any", parts=tuple(parts))

    @classmethod
    def by_schema(cls, schema_id: str) -> Self:
        return cls(type="by_schema", schema_id=schema_id)

    @classmethod
    def by_schema_prefix(cls, prefix: str) -> Self:
        return cls(type="by_schema_prefix", prefix=prefix)

    @classmethod
    def by_source_id(cls, source_id: str) -> Self:
        return cls(type="by_source_id", source_id=source_id)

    @classmethod
    def by_source_kind(cls, kind: str) -> Self:
        return cls(type="by_source_kind", kind=kind)

    @classmethod
    def by_capability(cls, capability: str) -> Self:
        return cls(type="by_capability", capability=capability)

    @classmethod
    def by_source_field(cls, field: str, value: ScalarValue) -> Self:
        return cls(type="by_source_field", field=field, value=value)

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "ScopeRuleSpec")
        rule_type = _as_str(_field(raw, "type"), "type")
        if rule_type in {"always", "never"}:
            return cls(type=rule_type)
        if rule_type in {"all", "any"}:
            parts = _field(raw, "parts")
            if not isinstance(parts, Sequence) or isinstance(parts, str | bytes | bytearray):
                raise TypeError("parts expects a sequence")
            decoded = tuple(
                cls.from_json_dict(_as_mapping(part, "ScopeRuleSpec")) for part in parts
            )
            return cls(type=rule_type, parts=decoded)
        if rule_type == "by_schema":
            return cls.by_schema(_as_str(_field(raw, "schema_id"), "schema_id"))
        if rule_type == "by_schema_prefix":
            return cls.by_schema_prefix(_as_str(_field(raw, "prefix"), "prefix"))
        if rule_type == "by_source_id":
            return cls.by_source_id(_as_str(_field(raw, "source_id"), "source_id"))
        if rule_type == "by_source_kind":
            return cls.by_source_kind(_as_str(_field(raw, "kind"), "kind"))
        if rule_type == "by_capability":
            return cls.by_capability(_as_str(_field(raw, "capability"), "capability"))
        if rule_type == "by_source_field":
            return cls.by_source_field(
                _as_str(_field(raw, "field"), "field"),
                _as_scalar(_field(raw, "value"), "value"),
            )
        raise ValueError(f"unknown scope rule type: {rule_type}")

    def to_json_dict(self) -> JsonDict:
        if self.type in {"always", "never"}:
            return {"type": self.type}
        if self.type in {"all", "any"}:
            return {"type": self.type, "parts": [_to_json_value(part) for part in self.parts]}
        if self.type == "by_schema":
            return {"type": self.type, "schema_id": self.schema_id}
        if self.type == "by_schema_prefix":
            return {"type": self.type, "prefix": self.prefix}
        if self.type == "by_source_id":
            return {"type": self.type, "source_id": self.source_id}
        if self.type == "by_source_kind":
            return {"type": self.type, "kind": self.kind}
        if self.type == "by_capability":
            return {"type": self.type, "capability": self.capability}
        if self.type == "by_source_field":
            return {"type": self.type, "field": self.field, "value": self.value}
        raise ValueError(f"unknown scope rule type: {self.type}")

    def matches(self, envelope: Envelope) -> bool:
        if self.type == "always":
            return True
        if self.type == "never":
            return False
        if self.type == "all":
            return all(part.matches(envelope) for part in self.parts)
        if self.type == "any":
            return any(part.matches(envelope) for part in self.parts)
        if self.type == "by_schema":
            return envelope.payload_schema_id == self.schema_id
        if self.type == "by_schema_prefix":
            return envelope.payload_schema_id.startswith(self.prefix)
        if self.type == "by_source_id":
            return envelope.source.source_id == self.source_id
        if self.type == "by_source_kind":
            return envelope.source.kind == self.kind
        if self.type == "by_capability":
            return self.capability in envelope.capabilities_required
        if self.type == "by_source_field":
            return envelope.source.metadata.get(self.field) == self.value
        return False


@dataclass(frozen=True)
class SourceRef:
    source_id: str
    kind: str
    metadata: dict[str, ScalarValue] = field(default_factory=dict)

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "SourceRef")
        return cls(
            source_id=_as_str(_field(raw, "source_id"), "source_id"),
            kind=_as_str(_field(raw, "kind"), "kind"),
            metadata=_as_scalar_dict(_field(raw, "metadata"), "metadata"),
        )


@dataclass(frozen=True)
class Envelope:
    id: str
    timestamp: float
    source: SourceRef
    payload_schema_id: str = ""
    capabilities_required: tuple[str, ...] = ()
    payload: JsonValue = None

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "Envelope")
        source = _field(raw, "source")
        return cls(
            id=_as_str(_field(raw, "id"), "id"),
            timestamp=_as_float(_field(raw, "timestamp"), "timestamp"),
            source=SourceRef.from_json_dict(_as_mapping(source, "source")),
            payload_schema_id=_as_str(_field(raw, "payload_schema_id"), "payload_schema_id"),
            capabilities_required=_as_str_tuple(
                _field(raw, "capabilities_required"), "capabilities_required"
            ),
            payload=_as_json_value(_field(raw, "payload")),
        )


@dataclass(frozen=True)
class AgentSpec:
    agent_id: str
    owner: str | None = None
    priority: int = 0
    participation: AgentParticipation = AgentParticipation.PRIMARY_CANDIDATE
    accepts: tuple[ScopeRuleSpec, ...] = ()
    strategy_id: str = ""
    side_effect_policy: SideEffectPolicy = SideEffectPolicy.READ_ONLY

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "AgentSpec")
        raw_accepts = _field(raw, "accepts")
        if not isinstance(raw_accepts, Sequence) or isinstance(
            raw_accepts, str | bytes | bytearray
        ):
            raise TypeError("accepts expects a sequence")
        owner = _field(raw, "owner")
        return cls(
            agent_id=_as_str(_field(raw, "agent_id"), "agent_id"),
            owner=None if owner is None else _as_str(owner, "owner"),
            priority=_as_int(_field(raw, "priority"), "priority"),
            participation=AgentParticipation(
                _as_str(_field(raw, "participation"), "participation")
            ),
            accepts=tuple(
                ScopeRuleSpec.from_json_dict(_as_mapping(item, "ScopeRuleSpec"))
                for item in raw_accepts
            ),
            strategy_id=_as_str(_field(raw, "strategy_id"), "strategy_id"),
            side_effect_policy=SideEffectPolicy(
                _as_str(_field(raw, "side_effect_policy"), "side_effect_policy")
            ),
        )


@dataclass(frozen=True)
class AgentSnapshot:
    spec: AgentSpec
    phase: AgentPhase
    inbox_len: int

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "AgentSnapshot")
        return cls(
            spec=AgentSpec.from_json_dict(_as_mapping(_field(raw, "spec"), "spec")),
            phase=AgentPhase(_as_str(_field(raw, "phase"), "phase")),
            inbox_len=_as_int(_field(raw, "inbox_len"), "inbox_len"),
        )


@dataclass(frozen=True)
class OperationDescriptor:
    op_id: str
    name: str
    description: str = ""
    plugin_id: str = ""
    func_qualname: str = ""
    parameters_schema: JsonDict = field(default_factory=dict)
    return_schema: JsonDict = field(default_factory=dict)
    perms_rule_id: str | None = None
    requires_capabilities: tuple[str, ...] = ()
    is_tool: bool = True

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "OperationDescriptor")
        perms_rule_id = _field(raw, "perms_rule_id")
        return cls(
            op_id=_as_str(_field(raw, "op_id"), "op_id"),
            name=_as_str(_field(raw, "name"), "name"),
            description=_as_str(_field(raw, "description"), "description"),
            plugin_id=_as_str(_field(raw, "plugin_id"), "plugin_id"),
            func_qualname=_as_str(_field(raw, "func_qualname"), "func_qualname"),
            parameters_schema=_as_json_dict(_field(raw, "parameters_schema"), "parameters_schema"),
            return_schema=_as_json_dict(_field(raw, "return_schema"), "return_schema"),
            perms_rule_id=None
            if perms_rule_id is None
            else _as_str(perms_rule_id, "perms_rule_id"),
            requires_capabilities=_as_str_tuple(
                _field(raw, "requires_capabilities"), "requires_capabilities"
            ),
            is_tool=_as_bool(_field(raw, "is_tool"), "is_tool"),
        )


@dataclass(frozen=True)
class SourceDescriptor:
    source_id: str
    kind: str
    capabilities: tuple[str, ...] = ()
    description: str = ""

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "SourceDescriptor")
        return cls(
            source_id=_as_str(_field(raw, "source_id"), "source_id"),
            kind=_as_str(_field(raw, "kind"), "kind"),
            capabilities=_as_str_tuple(_field(raw, "capabilities"), "capabilities"),
            description=_as_str(_field(raw, "description"), "description"),
        )


@dataclass(frozen=True)
class OperationHandlerKey:
    plugin_id: str
    plugin_generation: int
    op_id: str
    handler_id: str

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "OperationHandlerKey")
        return cls(
            plugin_id=_as_str(_field(raw, "plugin_id"), "plugin_id"),
            plugin_generation=_as_int(_field(raw, "plugin_generation"), "plugin_generation"),
            op_id=_as_str(_field(raw, "op_id"), "op_id"),
            handler_id=_as_str(_field(raw, "handler_id"), "handler_id"),
        )


@dataclass(frozen=True)
class OperationSnapshot:
    descriptor: OperationDescriptor
    status: OperationStatus
    key: OperationHandlerKey

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "OperationSnapshot")
        return cls(
            descriptor=OperationDescriptor.from_json_dict(
                _as_mapping(_field(raw, "descriptor"), "descriptor")
            ),
            status=OperationStatus(_as_str(_field(raw, "status"), "status")),
            key=OperationHandlerKey.from_json_dict(_as_mapping(_field(raw, "key"), "key")),
        )


@dataclass(frozen=True)
class SourceSnapshot:
    descriptor: SourceDescriptor
    plugin_id: str
    plugin_generation: int

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "SourceSnapshot")
        return cls(
            descriptor=SourceDescriptor.from_json_dict(
                _as_mapping(_field(raw, "descriptor"), "descriptor")
            ),
            plugin_id=_as_str(_field(raw, "plugin_id"), "plugin_id"),
            plugin_generation=_as_int(_field(raw, "plugin_generation"), "plugin_generation"),
        )


@dataclass(frozen=True)
class PluginDescriptor:
    plugin_id: str
    generation: int
    name: str
    description: str = ""
    version: str = ""
    capabilities: tuple[str, ...] = ()
    metadata: dict[str, ScalarValue] = field(default_factory=dict)

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "PluginDescriptor")
        return cls(
            plugin_id=_as_str(_field(raw, "plugin_id"), "plugin_id"),
            generation=_as_int(_field(raw, "generation"), "generation"),
            name=_as_str(_field(raw, "name"), "name"),
            description=_as_str(_field(raw, "description"), "description"),
            version=_as_str(_field(raw, "version"), "version"),
            capabilities=_as_str_tuple(_field(raw, "capabilities"), "capabilities"),
            metadata=_as_scalar_dict(_field(raw, "metadata"), "metadata"),
        )


@dataclass(frozen=True)
class PluginSnapshot:
    descriptor: PluginDescriptor
    status: PluginStatus

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "PluginSnapshot")
        return cls(
            descriptor=PluginDescriptor.from_json_dict(
                _as_mapping(_field(raw, "descriptor"), "descriptor")
            ),
            status=PluginStatus(_as_str(_field(raw, "status"), "status")),
        )


@dataclass(frozen=True)
class PluginAccessState:
    enabled_plugin_ids: tuple[str, ...] = ()
    disabled_plugin_ids: tuple[str, ...] = ()

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "PluginAccessState")
        return cls(
            enabled_plugin_ids=_as_str_tuple(
                _field(raw, "enabled_plugin_ids"), "enabled_plugin_ids"
            ),
            disabled_plugin_ids=_as_str_tuple(
                _field(raw, "disabled_plugin_ids"), "disabled_plugin_ids"
            ),
        )


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
        lost_capability = _field(raw, "lost_capability")
        recovery = _field(raw, "recovery")
        cause = _field(raw, "cause")
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
class StrategyResult:
    status: StrategyResultStatus
    decision: JsonValue = None
    emitted: tuple[Envelope, ...] = ()
    error: RuntimeError | None = None

    @classmethod
    def wait_input(cls) -> Self:
        return cls(status=StrategyResultStatus.WAIT_INPUT)

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "StrategyResult")
        raw_emitted = _field(raw, "emitted")
        if not isinstance(raw_emitted, Sequence) or isinstance(
            raw_emitted, str | bytes | bytearray
        ):
            raise TypeError("emitted expects a sequence")
        error = _field(raw, "error")
        return cls(
            status=StrategyResultStatus(_as_str(_field(raw, "status"), "status")),
            decision=_as_json_value(_field(raw, "decision")),
            emitted=tuple(
                Envelope.from_json_dict(_as_mapping(item, "Envelope")) for item in raw_emitted
            ),
            error=None
            if error is None
            else RuntimeError.from_json_dict(_as_mapping(error, "error")),
        )


@dataclass(frozen=True)
class RefDescriptor:
    ref_id: str
    kind: str
    schema_id_target: str
    schema_version_target: str
    attributes: dict[str, ScalarValue] = field(default_factory=dict)
    lineage: tuple[str, ...] = ()

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "RefDescriptor")
        return cls(
            ref_id=_as_str(_field(raw, "ref_id"), "ref_id"),
            kind=_as_str(_field(raw, "kind"), "kind"),
            schema_id_target=_as_str(_field(raw, "schema_id_target"), "schema_id_target"),
            schema_version_target=_as_str(
                _field(raw, "schema_version_target"), "schema_version_target"
            ),
            attributes=_as_scalar_dict(_field(raw, "attributes"), "attributes"),
            lineage=_as_str_tuple(_field(raw, "lineage"), "lineage"),
        )


@dataclass(frozen=True)
class LeaseToken:
    token_id: str
    ref_id: str
    owner: str

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "LeaseToken")
        return cls(
            token_id=_as_str(_field(raw, "token_id"), "token_id"),
            ref_id=_as_str(_field(raw, "ref_id"), "ref_id"),
            owner=_as_str(_field(raw, "owner"), "owner"),
        )


@dataclass(frozen=True)
class ResourceRecord:
    descriptor: RefDescriptor
    owner: str
    lease_count: int = 0

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "ResourceRecord")
        return cls(
            descriptor=RefDescriptor.from_json_dict(
                _as_mapping(_field(raw, "descriptor"), "descriptor")
            ),
            owner=_as_str(_field(raw, "owner"), "owner"),
            lease_count=_as_int(_field(raw, "lease_count"), "lease_count"),
        )


@dataclass(frozen=True)
class TraceSpan:
    trace_id: str
    span_id: str
    parent_span_id: str | None = None
    name: str = ""
    start: float = 0.0
    end: float | None = None
    attributes: dict[str, ScalarValue] = field(default_factory=dict)
    status: SpanStatus = SpanStatus.OK

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "TraceSpan")
        end = _field(raw, "end")
        parent_span_id = _field(raw, "parent_span_id")
        return cls(
            trace_id=_as_str(_field(raw, "trace_id"), "trace_id"),
            span_id=_as_str(_field(raw, "span_id"), "span_id"),
            parent_span_id=None
            if parent_span_id is None
            else _as_str(parent_span_id, "parent_span_id"),
            name=_as_str(_field(raw, "name"), "name"),
            start=_as_float(_field(raw, "start"), "start"),
            end=None if end is None else _as_float(end, "end"),
            attributes=_as_scalar_dict(_field(raw, "attributes"), "attributes"),
            status=SpanStatus(_as_str(_field(raw, "status"), "status")),
        )


@dataclass(frozen=True)
class RuntimeEvent:
    sequence: int
    kind: RuntimeEventKind
    name: str
    agent_id: str | None = None
    attributes: dict[str, ScalarValue] = field(default_factory=dict)
    error: RuntimeError | None = None

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = _as_mapping(data, "RuntimeEvent")
        error = _field(raw, "error")
        agent_id = _field(raw, "agent_id")
        return cls(
            sequence=_as_int(_field(raw, "sequence"), "sequence"),
            kind=RuntimeEventKind(_as_str(_field(raw, "kind"), "kind")),
            name=_as_str(_field(raw, "name"), "name"),
            agent_id=None if agent_id is None else _as_str(agent_id, "agent_id"),
            attributes=_as_scalar_dict(_field(raw, "attributes"), "attributes"),
            error=None
            if error is None
            else RuntimeError.from_json_dict(_as_mapping(error, "error")),
        )
