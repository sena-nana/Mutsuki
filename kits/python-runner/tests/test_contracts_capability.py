from __future__ import annotations

from mutsuki_runner_kit.contracts.capability_request import (
    CapabilityDescriptor,
    CapabilityRequestEnvelope,
    DeliveryReceipt,
    RejectionReason,
)
from mutsuki_runner_kit.contracts.codec import to_json_dict
from mutsuki_runner_kit.contracts.ids import CapabilityPeerId, CapabilityRequestId, TaskId
from mutsuki_runner_kit.testing.assertions import assert_json_roundtrip


def test_capability_envelope_and_delivery_receipt_roundtrip() -> None:
    envelope = CapabilityRequestEnvelope(
        request_id=CapabilityRequestId("req-1"),
        source=CapabilityPeerId("source.app"),
        target=CapabilityPeerId("target.app"),
        capability=CapabilityDescriptor(
            name="demo.capability",
            protocol_version=1,
            schema_version=1,
        ),
        payload={"ok": True},
    )
    assert_json_roundtrip(CapabilityRequestEnvelope, envelope)

    accepted = DeliveryReceipt.accepted("req-1", "task-1")
    decoded = assert_json_roundtrip(DeliveryReceipt, accepted)
    assert isinstance(decoded.request_id, CapabilityRequestId)
    assert isinstance(decoded.remote_task_id, TaskId)
    assert decoded.remote_task_id == "task-1"
    assert to_json_dict(accepted) == {
        "kind": "accepted",
        "request_id": "req-1",
        "remote_task_id": "task-1",
    }

    completed = DeliveryReceipt.completed("req-1", remote_task_id="task-1", output={"n": 1})
    assert_json_roundtrip(DeliveryReceipt, completed)
    assert_json_roundtrip(
        DeliveryReceipt,
        DeliveryReceipt.duplicate("req-1", accepted),
    )
    assert_json_roundtrip(
        DeliveryReceipt,
        DeliveryReceipt.rejected("req-1", RejectionReason.permission_denied()),
    )
    assert_json_roundtrip(
        DeliveryReceipt,
        DeliveryReceipt.failed("req-1", "task.unsupported", "boom", remote_task_id="task-1"),
    )
    assert_json_roundtrip(RejectionReason, RejectionReason.other("x", "y"))
