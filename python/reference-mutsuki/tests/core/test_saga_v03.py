"""v0.3: Saga carries structured compensation failures."""

from __future__ import annotations

import pytest

from mutsuki.contracts.error import Errs
from mutsuki.core.saga import Saga, SagaCompensationError


@pytest.mark.asyncio
async def test_saga_compensation_error_is_structured() -> None:
    saga = Saga(owner="saga-test")
    log: list[str] = []

    async def forward_ok() -> str:
        log.append("forward-ok")
        return "ok"

    async def compensate_bad() -> None:
        log.append("compensate-bad")
        raise RuntimeError("compensation failed")

    async def forward_bad() -> str:
        log.append("forward-bad")
        raise ValueError("primary failed")

    async def compensate_unused() -> None:
        log.append("compensate-unused")

    saga.add_step(forward_ok, compensate_bad, name="ok")
    saga.add_step(forward_bad, compensate_unused, name="bad")

    with pytest.raises(SagaCompensationError) as exc:
        await saga.run()

    assert exc.value.error.code == Errs.TRANSACTION_COMPENSATION_FAILED
    assert exc.value.error.evidence["owner"] == "saga-test"
    assert exc.value.error.evidence["compensation_failure_count"] == 1
    assert exc.value.error.evidence["original_exception_type"] == "ValueError"
    assert log == ["forward-ok", "forward-bad", "compensate-bad"]
