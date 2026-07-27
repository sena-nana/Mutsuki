from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from typing import Self

from mutsuki_runner_kit.contracts.codec import (
    JsonDict,
    JsonValue,
    as_int,
    as_json_value,
    as_mapping,
    as_str,
    field_value,
    optional_str,
    tuple_from_json,
)


@dataclass(frozen=True)
class EffectPrecondition:
    ref_id: str
    expected_version: int

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "EffectPrecondition")
        return cls(
            ref_id=as_str(field_value(raw, "ref_id"), "ref_id"),
            expected_version=as_int(field_value(raw, "expected_version"), "expected_version"),
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
        raw = as_mapping(data, "EffectRequest")
        return cls(
            effect_id=as_str(field_value(raw, "effect_id"), "effect_id"),
            kind=as_str(field_value(raw, "kind"), "kind"),
            payload=as_json_value(field_value(raw, "payload")),
            preconditions=tuple_from_json(raw, "preconditions", EffectPrecondition),
            idempotency_key=optional_str(field_value(raw, "idempotency_key"), "idempotency_key"),
        )
