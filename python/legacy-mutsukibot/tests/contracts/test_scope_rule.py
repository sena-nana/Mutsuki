"""ScopeRule —— AST 谓词组合 + 6 个 By* 叶子构造器（contracts §17）。

镜像 PermissionRule 的设计；与之最大差别是 ``check`` 同步（envelope 匹配
是纯数据计算）。
"""

from __future__ import annotations

import pytest

from mutsukibot.contracts import (
    ByCapability,
    BySchema,
    BySchemaPrefix,
    BySourceField,
    BySourceId,
    BySourceKind,
    Caps,
    Envelope,
    MessageId,
    ScopeName,
    ScopeRule,
    Scopes,
    SourceKindName,
    SourceKinds,
)
from mutsukibot_ext.im import ChannelRef, Message

BackendKind = SourceKindName.register("tests.backend", declared_by="tests")
AuditKind = SourceKindName.register("tests.audit", declared_by="tests")


def _msg(
    *,
    schema: str = "mutsukibot.message",
    source_id: str = "im:1",
    kind=SourceKinds.IM,
    caps=(),
    channel: str = "c",
) -> Envelope:
    ch = ChannelRef(source_id=source_id, kind=kind, channel_id=channel)
    return Message(
        id=MessageId("m"),
        timestamp=0.0,
        source=ch,
        payload_schema_id=schema,
        capabilities_required=tuple(caps),
    )


# ---------- Leaf constructors ----------


def test_by_schema_matches_exact() -> None:
    rule = BySchema("mutsukibot.message")
    assert rule.check(_msg(schema="mutsukibot.message"))
    assert not rule.check(_msg(schema="something.else"))


def test_by_schema_prefix_matches() -> None:
    rule = BySchemaPrefix("yume.")
    assert rule.check(_msg(schema="yume.thought"))
    assert rule.check(_msg(schema="yume."))
    assert not rule.check(_msg(schema="mutsukibot.message"))


def test_by_source_id_matches() -> None:
    rule = BySourceId("qq:bot1")
    assert rule.check(_msg(source_id="qq:bot1"))
    assert not rule.check(_msg(source_id="qq:bot2"))


def test_by_source_kind_matches() -> None:
    rule = BySourceKind(SourceKinds.IM)
    assert rule.check(_msg(kind=SourceKinds.IM))
    assert not rule.check(_msg(kind=BackendKind))


def test_by_capability_matches() -> None:
    rule = ByCapability(Caps.IM_TEXT)
    assert rule.check(_msg(caps=(Caps.IM_TEXT,)))
    assert rule.check(_msg(caps=(Caps.IM_IMAGE, Caps.IM_TEXT)))
    assert not rule.check(_msg(caps=(Caps.IM_IMAGE,)))


def test_by_source_field_matches_arbitrary_attr() -> None:
    rule = BySourceField("channel_id", "ops")
    assert rule.check(_msg(channel="ops"))
    assert not rule.check(_msg(channel="random"))


# ---------- AST combinators ----------


def test_and_requires_both() -> None:
    rule = BySchema("mutsukibot.message") & BySourceKind(SourceKinds.IM)
    assert rule.check(_msg(schema="mutsukibot.message", kind=SourceKinds.IM))
    assert not rule.check(_msg(schema="other", kind=SourceKinds.IM))
    assert not rule.check(_msg(schema="mutsukibot.message", kind=BackendKind))


def test_or_takes_either() -> None:
    rule = BySchema("a") | BySchema("b")
    assert rule.check(_msg(schema="a"))
    assert rule.check(_msg(schema="b"))
    assert not rule.check(_msg(schema="c"))


def test_complex_boolean_preserves_semantics() -> None:
    """`(A | B) & (C | D)` 必须严格按 (A OR B) AND (C OR D) 求值，
    不退化为四项 OR（与 PermissionRule 同语义）。"""
    rule = (BySchema("a") | BySchema("b")) & (
        BySourceKind(SourceKinds.IM) | BySourceKind(BackendKind)
    )
    assert rule.check(_msg(schema="a", kind=SourceKinds.IM))
    assert rule.check(_msg(schema="b", kind=BackendKind))
    # schema 在 (a, b) 但 kind 不在 (IM, BackendKind) —— AuditKind 不在 OR
    assert not rule.check(_msg(schema="a", kind=AuditKind))
    # kind 命中但 schema 不在
    assert not rule.check(_msg(schema="z", kind=SourceKinds.IM))


def test_always_and_never() -> None:
    assert ScopeRule.always().check(_msg())
    assert not ScopeRule.never().check(_msg())


def test_and_flattens_same_kind_nodes() -> None:
    """连续 AND 应平展，AST 不会无限嵌套。"""
    a = BySchema("a")
    b = BySchema("b")
    c = BySchema("c")
    flat = (a & b) & c
    # 内部 _And.parts 应该是 (a, b, c) 三元组而非嵌套 (_And(a, b), c)
    assert hasattr(flat, "parts")
    parts = getattr(flat, "parts")
    assert len(parts) == 3


# ---------- ScopeName facade ----------


def test_scopes_im_text_matches_im_text_message() -> None:
    rule = Scopes.IM_TEXT.to_rule()
    assert rule.check(
        _msg(schema="mutsukibot.message", kind=SourceKinds.IM, caps=(Caps.IM_TEXT,))
    )


def test_scopes_im_text_rejects_non_im() -> None:
    rule = Scopes.IM_TEXT.to_rule()
    assert not rule.check(
        _msg(schema="mutsukibot.message", kind=BackendKind, caps=(Caps.IM_TEXT,))
    )


def test_core_no_longer_exposes_tool_scopes_or_source_kinds() -> None:
    assert not hasattr(SourceKinds, "TOOL")
    assert not hasattr(SourceKinds, "HYBRID")
    assert not hasattr(Caps, "TOOL_INVOKE")
    assert not hasattr(Caps, "TOOL_EVENT")
    assert not hasattr(Scopes, "TOOL_INVOKE")
    assert not hasattr(Scopes, "TOOL_EVENT")


def test_scope_name_register_rejects_unknown_construction() -> None:
    """未注册的 ScopeName 构造时立即抛错（与 CapabilityName 同模式）。"""
    from mutsukibot.contracts.scope import UnknownScopeError

    with pytest.raises(UnknownScopeError):
        ScopeName("some.unregistered.scope")


def test_scope_name_register_returns_consistent_instance() -> None:
    """同 owner 重注册应该幂等。"""
    rule = BySchema("test.x")
    a = ScopeName.register("test.idempotent", declared_by="test", rule=rule)
    b = ScopeName.register("test.idempotent", declared_by="test", rule=rule)
    assert a == b
    assert a.to_rule().check(_msg(schema="test.x"))


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
