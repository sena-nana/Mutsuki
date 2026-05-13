"""Clock + IdGen 决定性行为。"""

from __future__ import annotations

import asyncio

import pytest

from nanobot.runtime import (
    DeterministicIdGen,
    ManualClock,
    ManualClockWaiterOverflow,
    NanoIdGen,
    SeededRng,
)


def test_deterministic_idgen_replays() -> None:
    a = DeterministicIdGen(seed=0)
    b = DeterministicIdGen(seed=0)
    for _ in range(20):
        assert a.next("x") == b.next("x")


def test_nano_idgen_unique_enough() -> None:
    g = NanoIdGen()
    seen = {g.next("x") for _ in range(1000)}
    assert len(seen) == 1000


@pytest.mark.asyncio
async def test_manual_clock_sleep_blocks_until_advance() -> None:
    clock = ManualClock(start=0)

    async def consumer() -> int:
        await clock.sleep(5.0)
        return 42

    task = asyncio.create_task(consumer())
    await asyncio.sleep(0.01)
    assert not task.done()
    clock.advance(5.0)
    result = await asyncio.wait_for(task, timeout=1.0)
    assert result == 42
    assert clock.now() == 5.0


def test_seeded_rng_reproducible() -> None:
    a = SeededRng(seed=42)
    b = SeededRng(seed=42)
    for _ in range(20):
        assert a.random() == b.random()


@pytest.mark.asyncio
async def test_manual_clock_advance_orders_waiters_by_deadline() -> None:
    """heap-based advance 应按到期顺序唤醒，不重建列表。"""
    clock = ManualClock(start=0)
    log: list[int] = []

    async def s(label: int, seconds: float) -> None:
        await clock.sleep(seconds)
        log.append(label)

    tasks = [
        asyncio.create_task(s(3, 3.0)),
        asyncio.create_task(s(1, 1.0)),
        asyncio.create_task(s(2, 2.0)),
    ]
    await asyncio.sleep(0.01)
    assert clock.pending_waiters == 3
    clock.advance(2.5)
    await asyncio.sleep(0.01)
    assert log == [1, 2]
    assert clock.pending_waiters == 1
    clock.advance(1.0)
    await asyncio.gather(*tasks)
    assert log == [1, 2, 3]
    assert clock.pending_waiters == 0


@pytest.mark.asyncio
async def test_manual_clock_warns_when_waiters_exceed_threshold() -> None:
    clock = ManualClock(max_pending_waiters=3)
    tasks = [asyncio.create_task(clock.sleep(10.0)) for _ in range(3)]
    await asyncio.sleep(0.01)
    overflow_task = asyncio.create_task(clock.sleep(10.0))
    with pytest.warns(ManualClockWaiterOverflow):
        await asyncio.sleep(0.01)
    assert clock.cancel_all() == 4
    await asyncio.gather(*tasks, overflow_task)


@pytest.mark.asyncio
async def test_manual_clock_cancel_all_releases_orphan_waiters() -> None:
    clock = ManualClock()
    t1 = asyncio.create_task(clock.sleep(100.0))
    t2 = asyncio.create_task(clock.sleep(200.0))
    await asyncio.sleep(0.01)
    assert clock.pending_waiters == 2
    cancelled = clock.cancel_all()
    assert cancelled == 2
    await asyncio.gather(t1, t2)
    assert clock.pending_waiters == 0
