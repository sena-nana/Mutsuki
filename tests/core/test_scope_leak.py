"""Scope 清理顺序 + 泄漏暴露。"""

from __future__ import annotations

import pytest

from nanobot.contracts.ids import RefId
from nanobot.core.handle import make_stub_handle
from nanobot.core.scope import HandleLeakError, PluginScope, TransactionScope


@pytest.mark.asyncio
async def test_cleanups_run_in_reverse_order() -> None:
    log: list[str] = []
    scope = PluginScope("p")
    scope.add_subscription(lambda: log.append("a"))
    scope.add_subscription(lambda: log.append("b"))
    scope.add_subscription(lambda: log.append("c"))
    await scope.close()
    assert log == ["c", "b", "a"]


@pytest.mark.asyncio
async def test_close_idempotent() -> None:
    scope = PluginScope("p")
    await scope.close()
    await scope.close()


@pytest.mark.asyncio
async def test_transaction_rollback_runs_compensations() -> None:
    log: list[str] = []
    tx = TransactionScope("tx")
    tx.register_compensation(lambda: log.append("comp-1"))
    tx.register_compensation(lambda: log.append("comp-2"))
    await tx.rollback()
    assert log == ["comp-2", "comp-1"]


@pytest.mark.asyncio
async def test_transaction_commit_skips_compensations() -> None:
    log: list[str] = []
    tx = TransactionScope("tx")
    tx.register_compensation(lambda: log.append("should-not-run"))
    await tx.commit()
    assert log == []


@pytest.mark.asyncio
async def test_leak_carries_error_with_correct_code() -> None:
    h = make_stub_handle(RefId("ref-leak"))
    scope = PluginScope("p")
    h.attach_to(scope)
    h.acquire()
    with pytest.raises(HandleLeakError) as exc:
        await scope.close()
    assert exc.value.error.code == "handle.leak"
