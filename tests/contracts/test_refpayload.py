"""Handle 生命周期 + 按引用 payload 的核心不变量。"""

from __future__ import annotations

import pytest

from nanobot.contracts.ids import RefId
from nanobot.core.handle import (
    HandleNotAttachedError,
    HandleUseAfterReleaseError,
    make_stub_handle,
)
from nanobot.core.scope import HandleLeakError, PluginScope


def test_handle_use_without_attach_raises() -> None:
    h = make_stub_handle(RefId("ref-1"))
    with pytest.raises(HandleNotAttachedError):
        h.acquire()


@pytest.mark.asyncio
async def test_scope_close_releases_attached_handle() -> None:
    h = make_stub_handle(RefId("ref-2"))
    scope = PluginScope("plugin-a")
    h.attach_to(scope)

    obj = h.acquire()
    assert obj is not None
    h.release()

    # 构造 +1，acquire +1，release -1，refcount=1。
    # 关闭 scope 应释放剩下的那一份。
    await scope.close()
    assert not h.is_alive()


@pytest.mark.asyncio
async def test_scope_close_detects_leaks() -> None:
    h = make_stub_handle(RefId("ref-3"))
    scope = PluginScope("plugin-leaker")
    h.attach_to(scope)

    h.acquire()  # refcount=2；close() 释放一次仍然存活

    with pytest.raises(HandleLeakError) as excinfo:
        await scope.close()
    assert RefId("ref-3") in excinfo.value.leaked


def test_handle_use_after_release_raises() -> None:
    h = make_stub_handle(RefId("ref-4"))
    scope = PluginScope("p")
    h.attach_to(scope)
    h.release()  # refcount 1 -> 0，已 finalized
    with pytest.raises(HandleUseAfterReleaseError):
        h.acquire()


@pytest.mark.asyncio
async def test_borrow_releases_on_context_exit() -> None:
    h = make_stub_handle(RefId("ref-5"), target="payload")
    scope = PluginScope("p")
    h.attach_to(scope)
    with h.borrow() as obj:
        assert obj == "payload"
    # 构造时的 +1 仍持有；scope 关闭时释放。
    await scope.close()


def test_refpayload_generic_specialization_does_not_pollute_schema_registry() -> None:
    """``RefPayload[T]`` 的参数化形式（如 ``RefPayload[Foo]``）必须复用同一个
    ``schema_id``，不得在 ``SchemaRegistry`` 里创造重复条目。

    这是 v0.5 引入领域插件（``RefPayload[LatentTensor]``）前的不变量保护。
    """
    from nanobot.contracts.base import SchemaRegistry
    from nanobot.contracts.refpayload import RefPayload

    class _Foo:
        pass

    class _Bar:
        pass

    # 参数化访问不能产生新的 Contract 子类（typing.Generic 的预期行为），
    # 所以注册表仍然只持有底类那一条。
    _ = RefPayload[_Foo]
    _ = RefPayload[_Bar]

    snapshot = SchemaRegistry.all()
    refpayload_entries = [
        (sid, ver, cls)
        for sid, ver, cls in snapshot
        if sid == "nanobot.ref_payload"
    ]
    assert len(refpayload_entries) == 1
    assert refpayload_entries[0][2] is RefPayload
