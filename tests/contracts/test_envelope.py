"""Envelope / SourceRef / Message 继承与字段语义。

验证 v0.2 引入的 Envelope 基类与 Message 继承关系（contracts §16）：

* Message 是 Envelope 子类
* ChannelRef 是 SourceRef 子类
* 领域 / 外部后端事件可由插件自定义 Envelope 子类
* schema_id 自动注册不冲突
* msgspec.Struct 多层继承构造正常
"""

from __future__ import annotations

from typing import ClassVar

import msgspec
import pytest

import mutsukibot.contracts as contracts
from mutsukibot.contracts import (
    Caps,
    ChannelRef,
    ContentKind,
    ContentPart,
    Envelope,
    EnvelopeId,
    Message,
    MessageId,
    SchemaRegistry,
    SourceKindName,
    SourceKinds,
    SourceRef,
)

BackendKind = SourceKindName.register("example.backend", declared_by="tests")


class BackendSourceRef(SourceRef):
    """测试用外部后端来源，由测试/插件侧自行定义而非核心内置。"""

    schema_id: ClassVar[str] = "tests.backend_source_ref"
    schema_version: ClassVar[str] = "1.0.0"

    stream_id: str = ""


class BackendEvent(Envelope):
    """测试用外部后端事件，由测试/插件侧自行定义而非核心内置。"""

    schema_id: ClassVar[str] = "tests.backend_event"
    schema_version: ClassVar[str] = "1.0.0"

    event_type: str = ""
    payload: dict[str, str] = msgspec.field(default_factory=dict)


def test_message_is_envelope_subclass() -> None:
    assert issubclass(Message, Envelope)
    assert issubclass(BackendEvent, Envelope)


def test_channel_ref_is_source_ref_subclass() -> None:
    assert issubclass(ChannelRef, SourceRef)
    assert issubclass(BackendSourceRef, SourceRef)


def test_message_id_is_envelope_id_alias() -> None:
    # MessageId = EnvelopeId in ids.py（NewType 别名），运行时同 str。
    msg_id = MessageId("m-1")
    env_id = EnvelopeId("e-1")
    assert isinstance(msg_id, str)
    assert isinstance(env_id, str)


def test_construct_channel_ref_with_required_fields() -> None:
    ch = ChannelRef(
        source_id="qq:bot1",
        kind=SourceKinds.IM,
        channel_id="c-1",
        user_id="u-1",
    )
    assert ch.source_id == "qq:bot1"
    assert ch.kind == SourceKinds.IM
    assert ch.channel_id == "c-1"


def test_construct_message_inherits_envelope_fields() -> None:
    ch = ChannelRef(source_id="im:1", kind=SourceKinds.IM, channel_id="c")
    m = Message(
        id=MessageId("msg-1"),
        timestamp=1.0,
        source=ch,
        payload_schema_id="mutsukibot.message",
        capabilities_required=(Caps.IM_TEXT,),
        parts=(ContentPart(kind=ContentKind.TEXT, text="hi"),),
    )
    # Envelope 字段
    assert m.id == "msg-1"
    assert m.timestamp == 1.0
    assert m.source is ch
    assert m.payload_schema_id == "mutsukibot.message"
    assert Caps.IM_TEXT in m.capabilities_required
    # Message 自有字段 + 便利属性
    assert len(m.parts) == 1
    assert m.text == "hi"


def test_construct_custom_backend_event() -> None:
    src = BackendSourceRef(source_id="backend:todo", kind=BackendKind, stream_id="todo")
    event = BackendEvent(
        id=EnvelopeId("be-1"),
        timestamp=2.0,
        source=src,
        payload_schema_id="example.todo.item_created",
        event_type="item_created",
        payload={"id": "item-1"},
    )
    assert event.event_type == "item_created"
    assert event.payload == {"id": "item-1"}
    assert isinstance(event.source, BackendSourceRef)


def test_envelope_subclass_schemas_registered() -> None:
    """所有 Envelope 子类的 schema_id 都已自动登记到 SchemaRegistry。"""
    assert SchemaRegistry.get("mutsukibot.envelope") is Envelope
    assert SchemaRegistry.get("mutsukibot.message") is Message
    assert SchemaRegistry.get("mutsukibot.source_ref") is SourceRef
    assert SchemaRegistry.get("mutsukibot.channel_ref") is ChannelRef
    assert SchemaRegistry.get("tests.backend_event") is BackendEvent
    assert SchemaRegistry.get("tests.backend_source_ref") is BackendSourceRef


def test_core_no_longer_exports_tool_event_contracts() -> None:
    assert not hasattr(contracts, "ToolEvent")
    assert not hasattr(contracts, "ToolSourceRef")


def test_message_text_property_concatenates_text_parts() -> None:
    ch = ChannelRef(source_id="im:1", kind=SourceKinds.IM, channel_id="c")
    m = Message(
        id=MessageId("m"),
        timestamp=0.0,
        source=ch,
        parts=(
            ContentPart(kind=ContentKind.TEXT, text="hello "),
            ContentPart(kind=ContentKind.IMAGE_REF, text=None),
            ContentPart(kind=ContentKind.TEXT, text="world"),
        ),
    )
    assert m.text == "hello world"


def test_envelope_default_capabilities_required_is_empty() -> None:
    ch = ChannelRef(source_id="im:1", kind=SourceKinds.IM, channel_id="c")
    m = Message(id=MessageId("m"), timestamp=0.0, source=ch)
    assert m.capabilities_required == ()
    assert m.parts == ()


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
