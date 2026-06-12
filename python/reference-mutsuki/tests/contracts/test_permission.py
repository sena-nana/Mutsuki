"""PermissionRule 组合 + 命名权限注册。"""

from __future__ import annotations

import pytest

from mutsuki.contracts.permission import (
    PermissionConflictError,
    PermissionName,
    PermissionRule,
    UnknownPermissionError,
)
from mutsuki.contracts.permission_builtin import Perms


@pytest.mark.asyncio
async def test_public_always_passes() -> None:
    rule = Perms.PUBLIC.to_rule()
    assert await rule.check(None)  # type: ignore[arg-type]


@pytest.mark.asyncio
async def test_and_compose_short_circuits_to_false() -> None:
    async def yes(_ctx: object) -> bool:
        return True

    async def no(_ctx: object) -> bool:
        return False

    rule = PermissionRule.from_checker(yes) & PermissionRule.from_checker(no)
    assert not await rule.check(None)  # type: ignore[arg-type]


@pytest.mark.asyncio
async def test_or_compose_passes_when_any_branch_passes() -> None:
    async def yes(_ctx: object) -> bool:
        return True

    async def no(_ctx: object) -> bool:
        return False

    rule = PermissionRule.from_checker(no) | PermissionRule.from_checker(yes)
    assert await rule.check(None)  # type: ignore[arg-type]


@pytest.mark.asyncio
async def test_or_and_or_keeps_distributive_semantics() -> None:
    """`(a|b) & (c|d)` 必须严格按 (a OR b) AND (c OR d) 求值，不退化为四项 OR。"""

    def fixed(value: bool) -> "object":
        async def _f(_ctx: object) -> bool:
            return value

        return _f

    def rule(left: bool, right: bool) -> PermissionRule:
        return PermissionRule.from_checker(fixed(left)) | PermissionRule.from_checker(fixed(right))  # type: ignore[arg-type]

    # (T|T) & (T|T) -> T
    assert await (rule(True, True) & rule(True, True)).check(None)  # type: ignore[arg-type]
    # (T|F) & (F|T) -> T
    assert await (rule(True, False) & rule(False, True)).check(None)  # type: ignore[arg-type]
    # (F|F) & (T|T) -> F (左侧 OR 全 False)
    assert not await (rule(False, False) & rule(True, True)).check(None)  # type: ignore[arg-type]
    # (T|T) & (F|F) -> F (右侧 OR 全 False)
    assert not await (rule(True, True) & rule(False, False)).check(None)  # type: ignore[arg-type]


def test_unknown_permission_rejected() -> None:
    with pytest.raises(UnknownPermissionError):
        PermissionName("definitely.not.registered.perm")


def test_owner_conflict_rejected() -> None:
    async def chk(_ctx: object) -> bool:
        return True

    PermissionName.register("test.perm.conflict", declared_by="A", checker=chk)
    with pytest.raises(PermissionConflictError):
        PermissionName.register("test.perm.conflict", declared_by="B", checker=chk)
