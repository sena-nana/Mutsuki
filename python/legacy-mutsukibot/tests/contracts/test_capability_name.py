"""CapabilityName 注册语义。"""

from __future__ import annotations

import pytest

from mutsukibot.contracts.capability import (
    Capability,
    CapabilityConflictError,
    CapabilityName,
    UnknownCapabilityError,
)
from mutsukibot.contracts.capability_builtin import Caps


def test_builtin_constants_registered() -> None:
    assert isinstance(Caps.READ_MESSAGE, CapabilityName)
    assert Caps.READ_MESSAGE == "read_message"
    assert CapabilityName.owner_of("read_message") == "mutsukibot.core"


def test_construction_requires_registration() -> None:
    with pytest.raises(UnknownCapabilityError):
        CapabilityName("definitely.not.registered.xyz")


def test_register_idempotent_for_same_owner() -> None:
    a = CapabilityName.register("test.capability.idem", declared_by="test-owner")
    b = CapabilityName.register("test.capability.idem", declared_by="test-owner")
    assert a is b


def test_register_conflict_across_owners() -> None:
    CapabilityName.register("test.capability.conflict", declared_by="owner-a")
    with pytest.raises(CapabilityConflictError):
        CapabilityName.register("test.capability.conflict", declared_by="owner-b")


def test_capability_struct_constructs_from_registered_name() -> None:
    cap = Capability(name=Caps.SEND_MESSAGE, quantity={"per_min": 30})
    assert cap.name == "send_message"
    assert cap.quantity == {"per_min": 30}
