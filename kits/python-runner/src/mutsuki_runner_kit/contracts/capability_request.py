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
    optional_id,
    optional_int,
    to_json_value,
)
from mutsuki_runner_kit.contracts.ids import CapabilityPeerId, CapabilityRequestId, TaskId

__all__ = (
    "CapabilityDescriptor",
    "CapabilityRequestEnvelope",
    "DeliveryReceipt",
    "RejectionReason",
)


@dataclass(frozen=True)
class CapabilityDescriptor:
    name: str
    protocol_version: int
    schema_version: int

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "CapabilityDescriptor")
        return cls(
            name=as_str(field_value(raw, "name"), "name"),
            protocol_version=as_int(field_value(raw, "protocol_version"), "protocol_version"),
            schema_version=as_int(field_value(raw, "schema_version"), "schema_version"),
        )


@dataclass(frozen=True)
class CapabilityRequestEnvelope:
    request_id: CapabilityRequestId
    source: CapabilityPeerId
    target: CapabilityPeerId
    capability: CapabilityDescriptor
    payload: JsonValue
    deadline_unix_ms: int | None = None

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "CapabilityRequestEnvelope")
        return cls(
            request_id=CapabilityRequestId(as_str(field_value(raw, "request_id"), "request_id")),
            source=CapabilityPeerId(as_str(field_value(raw, "source"), "source")),
            target=CapabilityPeerId(as_str(field_value(raw, "target"), "target")),
            capability=CapabilityDescriptor.from_json_dict(
                as_mapping(field_value(raw, "capability"), "capability")
            ),
            payload=as_json_value(field_value(raw, "payload")),
            deadline_unix_ms=optional_int(raw.get("deadline_unix_ms"), "deadline_unix_ms"),
        )

    def to_json_value(self) -> JsonDict:
        encoded: JsonDict = {
            "request_id": self.request_id,
            "source": self.source,
            "target": self.target,
            "capability": to_json_value(self.capability),
            "payload": as_json_value(self.payload),
        }
        if self.deadline_unix_ms is not None:
            encoded["deadline_unix_ms"] = self.deadline_unix_ms
        return encoded


@dataclass(frozen=True)
class RejectionReason:
    kind: str
    code: str | None = None
    message: str | None = None

    @classmethod
    def capability_unavailable(cls) -> Self:
        return cls(kind="capability_unavailable")

    @classmethod
    def protocol_incompatible(cls) -> Self:
        return cls(kind="protocol_incompatible")

    @classmethod
    def permission_denied(cls) -> Self:
        return cls(kind="permission_denied")

    @classmethod
    def payload_invalid(cls) -> Self:
        return cls(kind="payload_invalid")

    @classmethod
    def deadline_exceeded(cls) -> Self:
        return cls(kind="deadline_exceeded")

    @classmethod
    def cancelled(cls) -> Self:
        return cls(kind="cancelled")

    @classmethod
    def other(cls, code: str, message: str) -> Self:
        return cls(kind="other", code=code, message=message)

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        return cls.from_json_value(data)

    @classmethod
    def from_json_value(cls, value: object) -> Self:
        if isinstance(value, str):
            if value in {
                "capability_unavailable",
                "protocol_incompatible",
                "permission_denied",
                "payload_invalid",
                "deadline_exceeded",
                "cancelled",
            }:
                return cls(kind=value)
            raise TypeError(f"unknown RejectionReason: {value}")
        raw = as_mapping(value, "RejectionReason")
        if set(raw.keys()) != {"other"}:
            raise TypeError("RejectionReason expects a unit string or {'other': {...}}")
        other = as_mapping(raw["other"], "other")
        return cls.other(
            as_str(field_value(other, "code"), "code"),
            as_str(field_value(other, "message"), "message"),
        )

    def to_json_value(self) -> JsonValue:
        if self.kind == "other":
            if self.code is None or self.message is None:
                raise TypeError("other rejection requires code and message")
            return {"other": {"code": self.code, "message": self.message}}
        return self.kind


@dataclass(frozen=True)
class DeliveryReceipt:
    kind: str
    request_id: CapabilityRequestId
    remote_task_id: TaskId | None = None
    previous: DeliveryReceipt | None = None
    reason: RejectionReason | None = None
    output: JsonValue = None
    code: str | None = None
    message: str | None = None

    @classmethod
    def accepted(
        cls, request_id: str, remote_task_id: str | None = None
    ) -> Self:
        return cls(
            kind="accepted",
            request_id=CapabilityRequestId(request_id),
            remote_task_id=None if remote_task_id is None else TaskId(remote_task_id),
        )

    @classmethod
    def duplicate(cls, request_id: str, previous: DeliveryReceipt) -> Self:
        return cls(
            kind="duplicate",
            request_id=CapabilityRequestId(request_id),
            previous=previous,
        )

    @classmethod
    def rejected(cls, request_id: str, reason: RejectionReason) -> Self:
        return cls(kind="rejected", request_id=CapabilityRequestId(request_id), reason=reason)

    @classmethod
    def completed(
        cls,
        request_id: str,
        *,
        remote_task_id: str | None = None,
        output: JsonValue = None,
    ) -> Self:
        return cls(
            kind="completed",
            request_id=CapabilityRequestId(request_id),
            remote_task_id=None if remote_task_id is None else TaskId(remote_task_id),
            output=output,
        )

    @classmethod
    def failed(
        cls,
        request_id: str,
        code: str,
        message: str,
        *,
        remote_task_id: str | None = None,
    ) -> Self:
        return cls(
            kind="failed",
            request_id=CapabilityRequestId(request_id),
            remote_task_id=None if remote_task_id is None else TaskId(remote_task_id),
            code=code,
            message=message,
        )

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "DeliveryReceipt")
        kind = as_str(field_value(raw, "kind"), "kind")
        request_id = CapabilityRequestId(as_str(field_value(raw, "request_id"), "request_id"))
        if kind == "accepted":
            return cls.accepted(
                request_id,
                optional_id(TaskId, raw.get("remote_task_id"), "remote_task_id"),
            )
        if kind == "duplicate":
            return cls.duplicate(
                request_id,
                DeliveryReceipt.from_json_dict(
                    as_mapping(field_value(raw, "previous"), "previous")
                ),
            )
        if kind == "rejected":
            return cls.rejected(
                request_id,
                RejectionReason.from_json_value(field_value(raw, "reason")),
            )
        if kind == "completed":
            return cls.completed(
                request_id,
                remote_task_id=optional_id(TaskId, raw.get("remote_task_id"), "remote_task_id"),
                output=as_json_value(raw.get("output")),
            )
        if kind == "failed":
            return cls.failed(
                request_id,
                as_str(field_value(raw, "code"), "code"),
                as_str(field_value(raw, "message"), "message"),
                remote_task_id=optional_id(TaskId, raw.get("remote_task_id"), "remote_task_id"),
            )
        raise TypeError(f"unknown DeliveryReceipt kind: {kind}")

    def to_json_value(self) -> JsonDict:
        encoded: JsonDict = {"kind": self.kind, "request_id": self.request_id}
        if self.kind == "accepted":
            if self.remote_task_id is not None:
                encoded["remote_task_id"] = self.remote_task_id
            return encoded
        if self.kind == "duplicate":
            if self.previous is None:
                raise TypeError("previous is required for duplicate DeliveryReceipt")
            encoded["previous"] = to_json_value(self.previous)
            return encoded
        if self.kind == "rejected":
            if self.reason is None:
                raise TypeError("reason is required for rejected DeliveryReceipt")
            encoded["reason"] = to_json_value(self.reason)
            return encoded
        if self.kind == "completed":
            if self.remote_task_id is not None:
                encoded["remote_task_id"] = self.remote_task_id
            encoded["output"] = as_json_value(self.output)
            return encoded
        if self.kind == "failed":
            if self.code is None or self.message is None:
                raise TypeError("code and message are required for failed DeliveryReceipt")
            if self.remote_task_id is not None:
                encoded["remote_task_id"] = self.remote_task_id
            encoded["code"] = self.code
            encoded["message"] = self.message
            return encoded
        raise TypeError(f"unknown DeliveryReceipt kind: {self.kind}")
