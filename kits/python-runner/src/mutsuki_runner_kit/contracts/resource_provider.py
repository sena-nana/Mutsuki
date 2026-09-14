"""Provider lifecycle wire DTOs (runtime-wire 1.4.0, issue #184).

These mirror the ABI provider boundary; they do not implement a provider or registry.
"""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from typing import Self

from .codec import (
    JsonDict,
    JsonValue,
    as_int,
    as_json_dict,
    as_json_value,
    as_mapping,
    as_str,
    field_value,
    sequence,
)
from .errors import RuntimeError
from .resource import ResourceRef
from .resource_plans import (
    CommandBatch,
    CommandPlan,
    ExportPlan,
    PlanReceipt,
    ReadPlan,
    SagaPlan,
    SnapshotDescriptor,
    StreamPlan,
    WritePlan,
)


def _tagged(data: Mapping[str, object], names: set[str]) -> tuple[str, JsonValue]:
    if len(data) != 1:
        raise TypeError("provider enum expects one variant")
    variant = next(iter(data))
    if variant not in names:
        raise TypeError(f"unknown provider variant: {variant}")
    return variant, as_json_value(data[variant])


def _bytes(value: object) -> None:
    for byte in sequence(value, "bytes"):
        if not 0 <= as_int(byte, "byte") <= 255:
            raise TypeError("byte outside u8 range")


@dataclass(frozen=True)
class ResourceDescriptorInvalidation:
    provider_id: str
    ref_id: str
    generation: int

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object]) -> Self:
        generation = as_int(field_value(data, "generation"), "generation")
        if not 0 <= generation < 2**64:
            raise TypeError("generation outside u64 range")
        return cls(
            as_str(field_value(data, "provider_id"), "provider_id"),
            as_str(field_value(data, "ref_id"), "ref_id"),
            generation,
        )


@dataclass(frozen=True)
class ResourceProviderRequest:
    variant: str
    payload: JsonValue

    def to_json_value(self) -> JsonDict:
        return {self.variant: self.payload}

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object]) -> Self:
        variant, payload = _tagged(
            data,
            {
                "CreateBlob",
                "CreateCow",
                "CreateCapability",
                "Collect",
                "Snapshot",
                "OpenStream",
                "Export",
                "Commit",
                "Command",
                "Batch",
                "Saga",
            },
        )
        raw = as_mapping(payload, variant)
        if variant in {"CreateBlob", "CreateCow", "CreateCapability", "Snapshot"}:
            as_str(field_value(raw, "schema"), "schema")
            if variant != "CreateBlob":
                as_str(field_value(raw, "kind_id"), "kind_id")
        if variant in {"CreateBlob", "CreateCow", "Commit"}:
            _bytes(field_value(raw, "bytes"))
        if variant in {"Collect", "OpenStream"}:
            ReadPlan.from_json_dict(raw)
        elif variant == "Snapshot":
            ReadPlan.from_json_dict(as_mapping(field_value(raw, "plan"), "plan"))
        elif variant == "Export":
            ExportPlan.from_json_dict(raw)
        elif variant == "Commit":
            WritePlan.from_json_dict(as_mapping(field_value(raw, "plan"), "plan"))
        elif variant == "Command":
            CommandPlan.from_json_dict(raw)
        elif variant == "Batch":
            CommandBatch.from_json_dict(raw)
        elif variant == "Saga":
            SagaPlan.from_json_dict(raw)
        return cls(variant, payload)


@dataclass(frozen=True)
class ResourceProviderReply:
    variant: str
    payload: JsonValue

    def to_json_value(self) -> JsonDict:
        return {self.variant: self.payload}

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object]) -> Self:
        variant, payload = _tagged(
            data, {"Created", "Bytes", "Snapshot", "Stream", "Receipt", "Receipts"}
        )
        if variant == "Created":
            ResourceRef.from_json_dict(as_mapping(payload, variant))
        elif variant == "Bytes":
            _bytes(payload)
        elif variant == "Snapshot":
            SnapshotDescriptor.from_json_dict(as_mapping(payload, variant))
        elif variant == "Stream":
            StreamPlan.from_json_dict(as_mapping(payload, variant))
        elif variant == "Receipt":
            PlanReceipt.from_json_dict(as_mapping(payload, variant))
        else:
            for item in sequence(payload, variant):
                PlanReceipt.from_json_dict(as_mapping(item, "receipt"))
        return cls(variant, payload)


@dataclass(frozen=True)
class ResourceProviderResponse:
    result: JsonDict
    invalidations: tuple[ResourceDescriptorInvalidation, ...]

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object]) -> Self:
        result = as_json_dict(field_value(data, "result"), "result")
        variant, payload = _tagged(result, {"Ok", "Err"})
        if variant == "Ok":
            ResourceProviderReply.from_json_dict(as_mapping(payload, "reply"))
        else:
            RuntimeError.from_json_dict(as_mapping(payload, "error"))
        return cls(
            result,
            tuple(
                ResourceDescriptorInvalidation.from_json_dict(as_mapping(item, "invalidation"))
                for item in sequence(field_value(data, "invalidations"), "invalidations")
            ),
        )
