"""Dependent + Param 的认领/求解行为。"""

from __future__ import annotations

from typing import Annotated, cast

import pytest

from mutsukibot.contracts.error import Errs
from mutsukibot.contracts.ids import AgentId, RefId, SpanId, TraceId
from mutsukibot.contracts.plugin import Arg, RefArg, RefArgSource
from mutsukibot.contracts.refpayload import Handle
from mutsukibot.core.bus import Bus
from mutsukibot.core.container import ServiceContainer
from mutsukibot.core.context import AgentContext, TraceContext
from mutsukibot.core.dependency import (
    Dependent,
    RefResolutionError,
    UnresolvedParameterError,
)
from mutsukibot.core.dispatcher import Dispatcher
from mutsukibot.core.handle import make_stub_handle
from mutsukibot.core.resource_host import ResourceHost
from mutsukibot.core.scope import PluginScope
from mutsukibot.runtime import NanoIdGen, SeededRng, SystemClock


def _ctx() -> AgentContext:
    # 单测 Dependent 不真实路由，dispatcher 用 None 占位（cast 绕过类型）。
    return AgentContext(
        agent_id=AgentId("a"),
        agent_owner=None,
        clock=SystemClock(),
        id_gen=NanoIdGen(),
        rng=SeededRng(0),
        services=ServiceContainer(),
        scope=PluginScope("p"),
        bus=Bus(),
        dispatch=cast(Dispatcher, None),
        trace_ctx=TraceContext(trace_id=TraceId("t"), span_id=SpanId("s")),
    )


@pytest.mark.asyncio
async def test_ctx_param_resolves_agent_context() -> None:
    async def fn(ctx: AgentContext) -> str:
        return ctx.agent_id

    dep = Dependent.parse(fn)
    result = await dep.solve(_ctx())
    assert result == "a"


@pytest.mark.asyncio
async def test_arg_param_passes_through_extras() -> None:
    async def fn(ctx: AgentContext, text: str) -> str:
        return text.upper()

    dep = Dependent.parse(fn)
    result = await dep.solve(_ctx(), text="hello")
    assert result == "HELLO"


@pytest.mark.asyncio
async def test_annotated_constraints_dont_block_resolution() -> None:
    async def fn(ctx: AgentContext, n: Annotated[int, Arg(ge=1, le=10)] = 1) -> int:
        return n * 2

    dep = Dependent.parse(fn)
    assert await dep.solve(_ctx(), n=3) == 6


def test_unannotated_parameter_rejected() -> None:
    async def fn(ctx, text):  # type: ignore[no-untyped-def]
        return text

    with pytest.raises(UnresolvedParameterError):
        Dependent.parse(fn)


@pytest.mark.asyncio
async def test_refarg_payload_resolves_typed_handle_and_checks_kind() -> None:
    async def fn(
        resource: Annotated[Handle[dict[str, int]], RefArg(kind="test.resource")],
    ) -> int:
        with resource.borrow() as target:
            return target["value"]

    ctx = _ctx()
    scope = PluginScope("test-plugin")
    good = make_stub_handle(
        RefId("resource-1"),
        kind="test.resource",
        target={"value": 7},
    )
    good.attach_to(scope)
    bad = make_stub_handle(
        RefId("resource-2"),
        kind="test.other",
        target={"value": 9},
    )
    bad.attach_to(scope)

    dep = Dependent.parse(fn)
    assert await dep.solve(ctx, resource=good) == 7

    with pytest.raises(RefResolutionError) as exc:
        await dep.solve(ctx, resource=bad)
    assert exc.value.error.code == Errs.REF_KIND_MISMATCH
    assert exc.value.error.evidence["expected_kind"] == "test.resource"

    await scope.close()


@pytest.mark.asyncio
async def test_refarg_resource_host_resolves_handle_by_ref_id() -> None:
    async def fn(
        resource: Annotated[
            Handle[dict[str, int]],
            RefArg(
                kind="test.resource",
                source=RefArgSource.RESOURCE_HOST,
                ref_id="resource-1",
            ),
        ],
    ) -> int:
        with resource.borrow() as target:
            return target["value"]

    ctx = _ctx()
    host = ResourceHost(owner="test-host")
    ctx.services.register(ResourceHost, host)
    host.create_handle(
        RefId("resource-1"),
        target={"value": 11},
        kind="test.resource",
        schema_id_target="test.resource",
        schema_version_target="1.0.0",
    )

    dep = Dependent.parse(fn)
    assert await dep.solve(ctx) == 11

    await host.close()


@pytest.mark.asyncio
async def test_refarg_resource_host_missing_service_is_structured_error() -> None:
    async def fn(
        resource: Annotated[
            Handle[object],
            RefArg(
                kind="test.resource",
                source=RefArgSource.RESOURCE_HOST,
                ref_id="resource-1",
            ),
        ],
    ) -> object:
        return resource

    dep = Dependent.parse(fn)

    with pytest.raises(RefResolutionError) as exc:
        await dep.solve(_ctx())
    assert exc.value.error.code == Errs.SERVICE_NOT_FOUND
    assert exc.value.error.evidence["contract"] == "ResourceHost"
