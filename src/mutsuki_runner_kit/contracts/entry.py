from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from enum import StrEnum
from typing import Self

from mutsuki_runner_kit.contracts.codec import (
    JsonDict,
    as_mapping,
    as_str,
    field_value,
    optional_int,
)


class DispatchLane(StrEnum):
    CONTROL = "control"
    INTERACTIVE = "interactive"
    NORMAL = "normal"
    BACKGROUND = "background"
    BULK = "bulk"


class ResourceAccessMode(StrEnum):
    READ = "read"
    WRITE = "write"
    EXCLUSIVE_WRITE = "exclusive_write"


class PayloadLayout(StrEnum):
    ROW = "row"
    COLUMNAR = "columnar"
    BINARY_PACKED = "binary_packed"
    RESOURCE_BACKED = "resource_backed"


@dataclass(frozen=True)
class OrderingRequirement:
    type: str
    ref_id: str | None = None
    sequence_id: str | None = None

    @classmethod
    def none(cls) -> Self:
        return cls(type="none")

    @classmethod
    def preserve_submit_order(cls) -> Self:
        return cls(type="preserve_submit_order")

    @classmethod
    def same_resource_order(cls, ref_id: str) -> Self:
        return cls(type="same_resource_order", ref_id=ref_id)

    @classmethod
    def strict_sequence(cls, sequence_id: str) -> Self:
        return cls(type="strict_sequence", sequence_id=sequence_id)

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "OrderingRequirement")
        kind = as_str(field_value(raw, "type"), "type")
        if kind == "none":
            return cls.none()
        if kind == "preserve_submit_order":
            return cls.preserve_submit_order()
        if kind == "same_resource_order":
            return cls.same_resource_order(as_str(field_value(raw, "ref_id"), "ref_id"))
        if kind == "strict_sequence":
            return cls.strict_sequence(as_str(field_value(raw, "sequence_id"), "sequence_id"))
        raise TypeError(f"unknown OrderingRequirement type: {kind}")

    def to_json_value(self) -> JsonDict:
        if self.type == "none":
            return {"type": self.type}
        if self.type == "preserve_submit_order":
            return {"type": self.type}
        if self.type == "same_resource_order":
            if self.ref_id is None:
                raise TypeError("ref_id is required for same_resource_order")
            return {"type": self.type, "ref_id": self.ref_id}
        if self.type == "strict_sequence":
            if self.sequence_id is None:
                raise TypeError("sequence_id is required for strict_sequence")
            return {"type": self.type, "sequence_id": self.sequence_id}
        raise TypeError(f"unknown OrderingRequirement type: {self.type}")


@dataclass(frozen=True)
class ResourceRequirement:
    ref_id: str
    mode: ResourceAccessMode
    expected_version: int | None = None

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "ResourceRequirement")
        return cls(
            ref_id=as_str(field_value(raw, "ref_id"), "ref_id"),
            mode=ResourceAccessMode(as_str(field_value(raw, "mode"), "mode")),
            expected_version=optional_int(
                field_value(raw, "expected_version"), "expected_version"
            ),
        )
