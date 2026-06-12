"""ResourceHost 策略配置契约。

这一层只描述资源策略的 schema，不解释业务语义。策略只能观察通用
``ResourceRecord`` 字段：``ref_id``、``kind``、``schema_id_target``、
``schema_version_target``、``attributes``。是否反转匹配由 ``invert`` 显式声明。
"""

from __future__ import annotations

from typing import Any, ClassVar

from mutsuki.contracts.base import Contract
from mutsuki.contracts.ids import RefId


class ResourceRecordSelector(Contract):
    """用于 ResourceHost 策略的通用记录选择器。"""

    schema_id: ClassVar[str] = "mutsuki.resource_record_selector"
    schema_version: ClassVar[str] = "1.0.0"

    ref_id: RefId | None = None
    ref_id_prefix: str | None = None
    kind: str | None = None
    kind_prefix: str | None = None
    schema_id_target: str | None = None
    schema_id_target_prefix: str | None = None
    schema_version_target: str | None = None
    schema_version_target_prefix: str | None = None
    attributes: dict[str, str | int | float | bool] = {}
    invert: bool = False

    def is_empty(self) -> bool:
        return (
            self.ref_id is None
            and self.ref_id_prefix is None
            and self.kind is None
            and self.kind_prefix is None
            and self.schema_id_target is None
            and self.schema_id_target_prefix is None
            and self.schema_version_target is None
            and self.schema_version_target_prefix is None
            and not self.attributes
        )

    def matches(self, record: Any) -> bool:
        matched = True
        if self.ref_id is not None:
            matched = matched and getattr(record, "ref_id", None) == self.ref_id
        if self.ref_id_prefix is not None:
            matched = matched and str(getattr(record, "ref_id", "")).startswith(
                self.ref_id_prefix
            )
        if self.kind is not None:
            matched = matched and getattr(record, "kind", None) == self.kind
        if self.kind_prefix is not None:
            matched = matched and str(getattr(record, "kind", "")).startswith(
                self.kind_prefix
            )
        if self.schema_id_target is not None:
            matched = matched and getattr(record, "schema_id_target", None) == self.schema_id_target
        if self.schema_id_target_prefix is not None:
            matched = matched and str(
                getattr(record, "schema_id_target", "")
            ).startswith(self.schema_id_target_prefix)
        if self.schema_version_target is not None:
            matched = matched and getattr(
                record, "schema_version_target", None
            ) == self.schema_version_target
        if self.schema_version_target_prefix is not None:
            matched = matched and str(
                getattr(record, "schema_version_target", "")
            ).startswith(self.schema_version_target_prefix)
        if self.attributes:
            record_attributes = getattr(record, "attributes", {})
            matched = matched and all(
                record_attributes.get(key) == value
                for key, value in self.attributes.items()
            )
        return not matched if self.invert else matched


class ResourceHostPolicyConfig(Contract):
    """ResourceHost 的治理配置：分别描述 eviction / keepalive 策略。"""

    schema_id: ClassVar[str] = "mutsuki.resource_host_policy"
    schema_version: ClassVar[str] = "1.0.0"

    eviction: ResourceRecordSelector | None = None
    keepalive: ResourceRecordSelector | None = None

    def is_empty(self) -> bool:
        return self.eviction is None and self.keepalive is None


__all__ = [
    "ResourceHostPolicyConfig",
    "ResourceRecordSelector",
]
