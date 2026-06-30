from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from enum import StrEnum
from typing import Self

from mutsuki_runner_kit.contracts.codec import (
    JsonDict,
    JsonValue,
    as_int,
    as_json_value,
    as_mapping,
    as_str,
    field_value,
)


class ConflictPolicy(StrEnum):
    RETRY = "retry"
    MERGE = "merge"
    DISCARD = "discard"
    FAIL = "fail"
    EMIT_CONFLICT_TASK = "emit_conflict_task"


@dataclass(frozen=True)
class VersionExpectation:
    ref_id: str
    expected_version: int

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "VersionExpectation")
        return cls(
            ref_id=as_str(field_value(raw, "ref_id"), "ref_id"),
            expected_version=as_int(field_value(raw, "expected_version"), "expected_version"),
        )


@dataclass(frozen=True)
class StateRef:
    ref_id: str
    schema: str
    version: int

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "StateRef")
        return cls(
            ref_id=as_str(field_value(raw, "ref_id"), "ref_id"),
            schema=as_str(field_value(raw, "schema"), "schema"),
            version=as_int(field_value(raw, "version"), "version"),
        )


@dataclass(frozen=True)
class StateDelta:
    target_ref: str
    expected_version: int
    patch: JsonValue
    conflict_policy: ConflictPolicy

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "StateDelta")
        return cls(
            target_ref=as_str(field_value(raw, "target_ref"), "target_ref"),
            expected_version=as_int(field_value(raw, "expected_version"), "expected_version"),
            patch=as_json_value(field_value(raw, "patch")),
            conflict_policy=ConflictPolicy(
                as_str(field_value(raw, "conflict_policy"), "conflict_policy")
            ),
        )
