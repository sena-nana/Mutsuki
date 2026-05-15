"""Envelope / SourceRef / Message 继承与字段语义。

验证 v0.2 引入的 Envelope 基类与 Message 继承关系（contracts §16）：

* Message 是 Envelope 子类
* ChannelRef 是 SourceRef 子类
* ToolEvent 与 Message 共享 Envelope 接口
* schema_id 自动注册不冲突
* msgspec.Struct 多层继承构造正常
"""

from __future__ import annotations

import pytest

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
    SourceKinds,
    SourceRef,
    ToolEvent,
    ToolSourceRef,
)


def test_message_is_envelope_subclass() -> None:
    assert issubclass(Message, Envelope)
    assert issubclass(ToolEvent, Envelope)


def test_channel_ref_is_source_ref_subclass() -> None:
    assert issubclass(ChannelRef, SourceRef)
    assert issubclass(ToolSourceRef, SourceRef)


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


def test_construct_tool_event() -> None:
    src = ToolSourceRef(source_id="todo:default", kind=SourceKinds.TOOL)
    te = ToolEvent(
        id=EnvelopeId("te-1"),
        timestamp=2.0,
        source=src,
        payload_schema_id="mutsukibot.tool_event",
        event_type="todo.created",
        payload={"id": "item-1"},
    )
    assert te.event_type == "todo.created"
    assert te.payload == {"id": "item-1"}
    assert isinstance(te.source, ToolSourceRef)


def test_envelope_subclass_schemas_registered() -> None:
    """所有 Envelope 子类的 schema_id 都已自动登记到 SchemaRegistry。"""
    assert SchemaRegistry.get("mutsukibot.envelope") is Envelope
    assert SchemaRegistry.get("mutsukibot.message") is Message
    assert SchemaRegistry.get("mutsukibot.tool_event") is ToolEvent
    assert SchemaRegistry.get("mutsukibot.source_ref") is SourceRef
    assert SchemaRegistry.get("mutsukibot.channel_ref") is ChannelRef
    assert SchemaRegistry.get("mutsukibot.tool_source_ref") is ToolSourceRef


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
