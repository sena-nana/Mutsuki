"""Text command reference extension."""

from __future__ import annotations

import shlex
from typing import ClassVar

import msgspec

from mutsuki.contracts.capability import Capability
from mutsuki.contracts.error import Error, Errs
from mutsuki.contracts.event import SpanStatus, TraceSpan
from mutsuki.contracts.ids import MessageId, SpanId, TraceId
from mutsuki.core.container import ServiceNotFoundError
from mutsuki.core.dispatcher import OperationInvokeError
from mutsuki.core.plugin import Plugin, operation
from mutsuki.core.scope import HandleLeakError
from mutsuki_ext.im import (
    ChannelRef,
    ContentKind,
    ContentPart,
    IMScopes,
    IMSourceKinds,
    Message,
)


class _TextCommandRouterConfig(msgspec.Struct, kw_only=True):
    pass


class TextCommandRouterPlugin(Plugin[_TextCommandRouterConfig]):
    """Route IM text commands to registered Operations.

    This preserves the old echo-style text command behavior as an extension
    instead of keeping it inside the core scheduler.
    """

    id: ClassVar[str] = "mutsuki-ext-command-router"
    version: ClassVar[str] = "0.1.0"
    capabilities: ClassVar[list[Capability]] = []
    consumes: ClassVar[tuple] = (IMScopes.TEXT.to_rule(),)

    Config = _TextCommandRouterConfig

    async def on_envelope(self, envelope) -> None:  # type: ignore[override]
        if not isinstance(envelope, Message):
            return
        await route_text_command(self, envelope)


async def route_text_command(plugin: Plugin, msg: Message) -> bool:
    """Parse an IM text message and invoke the matching Operation if any."""

    text = msg.text.strip()
    if not text:
        return False
    try:
        tokens = shlex.split(text)
    except ValueError:
        tokens = text.split()
    if not tokens:
        return False

    cmd_name = tokens[0]
    positional = tokens[1:]
    agent = plugin.agent
    op_id = agent.dispatch.lookup_operation(cmd_name)
    if op_id is None:
        now = agent.clock.now()
        unmatched_span = TraceSpan(
            trace_id=TraceId(agent.id_gen.next("trace")),
            span_id=SpanId(agent.id_gen.next("span")),
            name="command.router.unmatched",
            start=now,
            end=now,
            status=SpanStatus.OK,
            attributes={
                "agent_id": agent.agent_id,
                "unmatched": True,
                "first_token": cmd_name,
            },
        )
        await agent.bus.publish("trace.span", unmatched_span)
        return False

    spec = next((op for op in agent.dispatch.list_operations() if op.op_id == op_id), None)
    if spec is None:
        return False

    param_names = list(spec.parameters_schema.get("properties", {}))
    payload: dict[str, object] = {}
    for name, value in zip(param_names, positional, strict=False):
        payload[name] = _coerce(value, spec.parameters_schema["properties"][name])

    ctx = agent.make_context(message=msg)
    try:
        result = await agent.dispatch.invoke(op_id, payload, ctx=ctx)
        await _emit_result(plugin, msg, str(result))
    except OperationInvokeError as exc:
        await _emit_error(plugin, msg, exc.error)
    except Exception as exc:
        await _emit_error(plugin, msg, _classify_command_exception(exc, spec.plugin_id, spec.name))
    return True


def _outbound_source(plugin: Plugin, inbound: Message) -> ChannelRef:
    src = inbound.source
    if isinstance(src, ChannelRef):
        return ChannelRef(
            source_id=src.source_id,
            kind=src.kind,
            channel_id=src.channel_id,
            user_id=src.user_id,
        )
    return ChannelRef(
        source_id=src.source_id,
        kind=IMSourceKinds.IM,
        channel_id=plugin.agent.agent_id,
    )


async def _emit_result(plugin: Plugin, msg: Message, text: str) -> None:
    out = Message(
        id=MessageId(plugin.agent.id_gen.next("msg")),
        timestamp=plugin.agent.clock.now(),
        source=_outbound_source(plugin, msg),
        payload_schema_id="mutsuki.message",
        parts=(ContentPart(kind=ContentKind.TEXT, text=text),),
    )
    await plugin.agent.outbox.put(out)


async def _emit_error(plugin: Plugin, msg: Message, err: Error) -> None:
    out = Message(
        id=MessageId(plugin.agent.id_gen.next("msg")),
        timestamp=plugin.agent.clock.now(),
        source=_outbound_source(plugin, msg),
        payload_schema_id="mutsuki.message",
        parts=(
            ContentPart(kind=ContentKind.TEXT, text=f"[error {err.code}] {err.evidence}"),
        ),
    )
    await plugin.agent.outbox.put(out)


def _coerce(raw: str, schema: dict[str, object]) -> object:
    t = schema.get("type")
    if t == "integer":
        return int(raw)
    if t == "number":
        return float(raw)
    if t == "boolean":
        return raw.lower() in {"true", "1", "yes"}
    return raw


def _classify_command_exception(exc: BaseException, plugin_id: str, command_name: str) -> Error:
    route = f"command.{command_name}"
    if isinstance(exc, HandleLeakError):
        return Error(
            code=Errs.HANDLE_LEAK,
            source=plugin_id,
            route=route,
            evidence=dict(exc.error.evidence),
        )
    if isinstance(exc, ServiceNotFoundError):
        return Error(
            code=Errs.SERVICE_NOT_FOUND,
            source=plugin_id,
            route=route,
            evidence={"reason": "service_not_found", "detail": str(exc)},
        )
    if isinstance(exc, KeyError):
        return Error(
            code=Errs.COMMAND_INVALID_ARGS,
            source=plugin_id,
            route=route,
            evidence={"reason": "missing_arg", "detail": str(exc)},
        )
    return Error(
        code=Errs.COMMAND_EXECUTION_FAILED,
        source=plugin_id,
        route=route,
        evidence={
            "reason": "command_raised",
            "exception_type": type(exc).__qualname__,
            "exception_repr": repr(exc),
        },
    )


command = operation

__all__ = ["TextCommandRouterPlugin", "command", "operation", "route_text_command"]
