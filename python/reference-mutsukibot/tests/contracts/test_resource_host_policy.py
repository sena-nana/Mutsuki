"""ResourceHost 策略契约测试。"""

from __future__ import annotations

from mutsukibot.contracts import RefId, SchemaRegistry
from mutsukibot.contracts.resource_host import (
    ResourceHostPolicyConfig,
    ResourceRecordSelector,
)
from mutsukibot.core.resource_host import ResourceRecord


def test_resource_host_policy_contracts_are_registered() -> None:
    assert SchemaRegistry.get("mutsukibot.resource_record_selector") is ResourceRecordSelector
    assert SchemaRegistry.get("mutsukibot.resource_host_policy") is ResourceHostPolicyConfig


def test_resource_record_selector_matches_generic_fields() -> None:
    selector = ResourceRecordSelector(
        kind="test.resource",
        schema_id_target_prefix="test.resource",
        attributes={"role": "primary"},
    )
    record = ResourceRecord(
        ref_id=RefId("resource-1"),
        kind="test.resource",
        schema_id_target="test.resource/v1",
        schema_version_target="1.0.0",
        attributes={"role": "primary", "size": "small"},
        last_touched_tick=3,
    )

    assert selector.matches(record)
    assert not ResourceRecordSelector(ref_id=RefId("dead")).matches(record)
    assert ResourceRecordSelector(ref_id=RefId("dead"), invert=True).matches(record)
