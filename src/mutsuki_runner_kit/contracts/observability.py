from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from enum import StrEnum
from typing import Self

from mutsuki_runner_kit.contracts.codec import (
    JsonDict,
    JsonValue,
    as_bool,
    as_int,
    as_json_value,
    as_mapping,
    as_str,
    field_value,
    optional_int,
    sequence,
)


class ObservabilityOverflowPolicy(StrEnum):
    DROP_OLDEST = "drop_oldest"
    DROP_NEW = "drop_new"


@dataclass(frozen=True)
class ObservabilityOutletProfile:
    capacity: int
    overflow_policy: ObservabilityOverflowPolicy

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "ObservabilityOutletProfile")
        return cls(
            capacity=as_int(field_value(raw, "capacity"), "capacity"),
            overflow_policy=ObservabilityOverflowPolicy(
                as_str(field_value(raw, "overflow_policy"), "overflow_policy")
            ),
        )


@dataclass(frozen=True)
class ObservabilityProfile:
    events: ObservabilityOutletProfile
    traces: ObservabilityOutletProfile
    detailed_scheduler_decisions: bool
    dispatch_spans: bool

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "ObservabilityProfile")
        return cls(
            events=ObservabilityOutletProfile.from_json_dict(
                as_mapping(field_value(raw, "events"), "events")
            ),
            traces=ObservabilityOutletProfile.from_json_dict(
                as_mapping(field_value(raw, "traces"), "traces")
            ),
            detailed_scheduler_decisions=as_bool(
                field_value(raw, "detailed_scheduler_decisions"),
                "detailed_scheduler_decisions",
            ),
            dispatch_spans=as_bool(field_value(raw, "dispatch_spans"), "dispatch_spans"),
        )


@dataclass(frozen=True)
class ObservabilityPage:
    items: tuple[JsonValue, ...]
    next_sequence: int
    earliest_available_sequence: int | None
    latest_sequence: int
    lost: int
    truncated: bool
    dropped: int

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "ObservabilityPage")
        return cls(
            items=tuple(
                as_json_value(item) for item in sequence(field_value(raw, "items"), "items")
            ),
            next_sequence=as_int(field_value(raw, "next_sequence"), "next_sequence"),
            earliest_available_sequence=optional_int(
                field_value(raw, "earliest_available_sequence"),
                "earliest_available_sequence",
            ),
            latest_sequence=as_int(field_value(raw, "latest_sequence"), "latest_sequence"),
            lost=as_int(field_value(raw, "lost"), "lost"),
            truncated=as_bool(field_value(raw, "truncated"), "truncated"),
            dropped=as_int(field_value(raw, "dropped"), "dropped"),
        )

    def cursor_lost(self) -> bool:
        return self.lost > 0
