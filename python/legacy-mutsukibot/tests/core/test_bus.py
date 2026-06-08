"""Bus 行为：deferred 异常隔离 + direct 透传 + 失败诊断聚合。"""

from __future__ import annotations

import pytest

from mutsukibot.core.bus import Bus


@pytest.mark.asyncio
async def test_deferred_handler_failure_does_not_block_others() -> None:
    """一个挂掉的 deferred handler 不能取消同事件的其他 handler。"""
    bus = Bus()
    received: list[object] = []

    async def good(payload: object) -> None:
        received.append(payload)

    async def bad(_payload: object) -> None:
        raise RuntimeError("trace writer 挂了")

    bus.subscribe("evt", bad)
    bus.subscribe("evt", good)

    # publish 不应抛异常，否则发布者（scheduler）会被拖死
    await bus.publish("evt", "ok")

    assert received == ["ok"]
    assert len(bus.failures) == 1
    assert bus.failures[0].event_type == "evt"
    assert bus.failures[0].exception_type == "RuntimeError"
    assert "trace writer 挂了" in bus.failures[0].exception_repr


@pytest.mark.asyncio
async def test_direct_handler_failure_propagates() -> None:
    """direct 订阅者契约就是不抛；抛了必须让 publisher 看到。"""
    bus = Bus()

    async def bad(_payload: object) -> None:
        raise ValueError("direct boom")

    bus.subscribe("evt", bad, direct=True)

    with pytest.raises(ValueError, match="direct boom"):
        await bus.publish("evt", None)

    # direct 路径不写 failures 列表（语义不同：direct 必须可见）
    assert bus.failures == []


@pytest.mark.asyncio
async def test_unsubscribe_removes_from_correct_bucket() -> None:
    bus = Bus()
    received: list[object] = []

    async def h(payload: object) -> None:
        received.append(payload)

    unsub = bus.subscribe("evt", h)
    await bus.publish("evt", 1)
    unsub()
    await bus.publish("evt", 2)

    assert received == [1]


@pytest.mark.asyncio
async def test_publish_to_unknown_event_is_noop() -> None:
    bus = Bus()
    await bus.publish("never-subscribed", "x")
    assert bus.failures == []
