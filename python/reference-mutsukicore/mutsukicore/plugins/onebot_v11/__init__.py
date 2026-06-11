"""OneBot v11 reverse WebSocket reference plugin.

This plugin is deliberately a reference transport plugin, not a core adapter.
All OneBot-specific wire fields stay in this module; core and contracts only
see Source, Operation, and Message contracts.
"""

from __future__ import annotations

import asyncio
from contextlib import suppress
import json
from typing import Any, ClassVar

import msgspec

from mutsukicore import Capability, Caps, Plugin
from mutsukicore.contracts import (
    Handle,
    MessageId,
    OperationDescriptor,
    Perms,
    RefDescriptor,
    RefId,
    SourceDescriptor,
)
from mutsukicore.contracts.error import Error, Errs
from mutsukicore.core.dispatcher import OperationInvokeError
from mutsukicore.core.handle import RefCountedHandle
from mutsukicore_ext.im import (
    ChannelRef,
    ContentKind,
    ContentPart,
    IMCaps,
    IMSourceKinds,
    Message,
)

_DEFAULT_SOURCE_ID = "onebot:v11.default"
_OP_SEND_MSG = OperationDescriptor(
    op_id=f"{_DEFAULT_SOURCE_ID}.send_msg",
    name="send_msg",
    description="Send a OneBot v11 message through the active reverse WS connection.",
    plugin_id="mutsukicore-onebot-v11",
    requires_capabilities=(Caps.SEND_MESSAGE, Caps.NETWORK_EGRESS),
    parameters_schema={
        "type": "object",
        "properties": {
            "message_type": {"type": "string"},
            "user_id": {"type": "integer"},
            "group_id": {"type": "integer"},
            "message": {"type": "string"},
            "auto_escape": {"type": "boolean"},
        },
        "required": ["message_type", "message"],
    },
    return_schema={"type": "object"},
)
_ONEBOT_SOURCE = SourceDescriptor(
    source_id=_DEFAULT_SOURCE_ID,
    kind=IMSourceKinds.IM,
    capabilities=(IMCaps.TEXT,),
    description="OneBot v11 reverse WebSocket IM source.",
)


class _OneBotV11Config(msgspec.Struct, kw_only=True):
    source_id: str = _DEFAULT_SOURCE_ID
    host: str = "127.0.0.1"
    port: int = 6701
    path: str = "/onebot/v11/ws"
    access_token: str | None = None
    request_timeout: float = 5.0


class OneBotV11Plugin(Plugin[_OneBotV11Config]):
    """Minimal OneBot v11 reverse WebSocket transport plugin."""

    id: ClassVar[str] = "mutsukicore-onebot-v11"
    version: ClassVar[str] = "0.2.0"
    capabilities: ClassVar[list[Capability]] = [
        Capability(name=Caps.READ_MESSAGE),
        Capability(name=Caps.SEND_MESSAGE),
        Capability(name=Caps.NETWORK_EGRESS),
    ]
    provides_operations: ClassVar[tuple[OperationDescriptor, ...]] = (_OP_SEND_MSG,)
    provides_sources: ClassVar[tuple[SourceDescriptor, ...]] = (_ONEBOT_SOURCE,)
    Config = _OneBotV11Config

    _server_handle: Handle[Any] | None
    _connection_handle: Handle[Any] | None
    _pending: dict[str, asyncio.Future[dict[str, Any]]]
    _outbox_task: asyncio.Task[None] | None
    _actual_port: int

    async def on_load(self) -> None:
        if self.config.source_id != _DEFAULT_SOURCE_ID:
            raise OperationInvokeError(
                Error(
                    code=Errs.PLUGIN_CONFIG_INVALID,
                    source=self.id,
                    route="onebot_v11.config",
                    evidence={
                        "reason": "dynamic_source_id_not_supported_in_v0_2",
                        "source_id": self.config.source_id,
                    },
                )
            )

        self._server_handle = None
        self._connection_handle = None
        self._pending = {}
        self._outbox_task = None
        self._actual_port = self.config.port

        self.agent.dispatch.register_source(
            _ONEBOT_SOURCE,
            plugin_scope=self.scope,
            plugin_id=self.id,
        )
        self.agent.dispatch.register_operation(
            _OP_SEND_MSG,
            handler=self._handle_send_msg,
            perms=Perms.PUBLIC.to_rule(),
            plugin_scope=self.scope,
        )

        try:
            from websockets.asyncio.server import serve
        except ModuleNotFoundError as exc:
            raise OperationInvokeError(
                Error(
                    code=Errs.PLUGIN_LOAD_FAILED,
                    source=self.id,
                    route="onebot_v11.import",
                    evidence={"reason": "websockets_extra_not_installed"},
                )
            ) from exc

        server = await serve(self._handle_connection, self.config.host, self.config.port)
        self._actual_port = int(server.sockets[0].getsockname()[1])
        self._server_handle = RefCountedHandle(
            target=server,
            descriptor=RefDescriptor(
                ref_id=RefId(f"{self.config.source_id}.server"),
                kind="onebot.v11.reverse_ws_server",
                schema_id_target="onebot.v11.reverse_ws_server",
                schema_version_target="1.0.0",
            ),
            finalizer=lambda target: target.close(),
        )
        self._server_handle.attach_to(self.scope)

        async def _close_server() -> None:
            server.close()
            await server.wait_closed()

        self.scope.add_dispose(_close_server)

        self._outbox_task = asyncio.create_task(self._outbox_pump())
        self.scope.add_timer(self._cancel_outbox_task)

    @property
    def url(self) -> str:
        return f"ws://{self.config.host}:{self._actual_port}{self.config.path}"

    @staticmethod
    def message_from_event(
        agent,
        *,
        source_id: str,
        event: dict[str, Any],
    ) -> Message:
        message_type = str(event.get("message_type", ""))
        if event.get("post_type") != "message" or message_type not in {"private", "group"}:
            raise ValueError("unsupported OneBot event")

        if message_type == "group":
            channel_id = f"group:{event['group_id']}"
        else:
            channel_id = f"private:{event['user_id']}"

        text, non_text_segments = _extract_text(event.get("message"))
        if not text:
            text = str(event.get("raw_message", ""))

        metadata = {
            "onebot.post_type": str(event.get("post_type", "")),
            "onebot.message_type": message_type,
        }
        if non_text_segments:
            metadata["onebot.non_text_segments"] = str(non_text_segments)

        msg_id = event.get("message_id")
        if msg_id is None:
            msg_id = agent.id_gen.next("msg")
        event_time = event.get("time")
        timestamp = (
            float(event_time)
            if isinstance(event_time, (int, float, str))
            else agent.clock.now()
        )
        return Message(
            id=MessageId(str(msg_id)),
            timestamp=timestamp,
            source=ChannelRef(
                source_id=source_id,
                kind=IMSourceKinds.IM,
                channel_id=channel_id,
                user_id=str(event.get("user_id")) if event.get("user_id") is not None else None,
            ),
            payload_schema_id="mutsukicore.message",
            capabilities_required=(IMCaps.TEXT,),
            parts=(ContentPart(kind=ContentKind.TEXT, text=text, metadata=metadata),),
        )

    async def _handle_connection(self, websocket: Any) -> None:
        if not self._authorized(websocket):
            await websocket.close(code=1008, reason="invalid access token")
            return

        conn_handle = RefCountedHandle(
            target=websocket,
            descriptor=RefDescriptor(
                ref_id=RefId(f"{self.config.source_id}.connection"),
                kind="onebot.v11.reverse_ws_connection",
                schema_id_target="onebot.v11.reverse_ws_connection",
                schema_version_target="1.0.0",
            ),
        )
        conn_handle.attach_to(self.scope)
        self._connection_handle = conn_handle
        try:
            async for raw in websocket:
                await self._handle_raw_frame(raw)
        finally:
            if self._connection_handle is conn_handle:
                self._connection_handle = None
            conn_handle.release()
            for future in self._pending.values():
                if not future.done():
                    future.set_exception(RuntimeError("onebot connection closed"))
            self._pending.clear()

    async def _handle_raw_frame(self, raw: str | bytes) -> None:
        try:
            frame = json.loads(raw)
        except json.JSONDecodeError:
            return
        if not isinstance(frame, dict):
            return
        echo = frame.get("echo")
        if echo is not None:
            future = self._pending.pop(str(echo), None)
            if future is not None and not future.done():
                future.set_result(frame)
            return
        if frame.get("post_type") == "message":
            try:
                msg = self.message_from_event(
                    self.agent,
                    source_id=self.config.source_id,
                    event=frame,
                )
            except ValueError:
                return
            await self.agent.dispatch.publish(msg)

    async def _handle_send_msg(self, _ctx, payload: dict[str, Any]) -> dict[str, Any]:
        handle = self._connection_handle
        if handle is None or not handle.is_alive():
            raise OperationInvokeError(
                Error(
                    code=Errs.OPERATION_INVOKE_FAILED,
                    source=self.id,
                    route=_OP_SEND_MSG.op_id,
                    evidence={"reason": "no_active_connection"},
                )
            )

        params = _send_params(payload)
        echo = self.agent.id_gen.next("onebot_echo")
        loop = asyncio.get_running_loop()
        future: asyncio.Future[dict[str, Any]] = loop.create_future()
        self._pending[str(echo)] = future
        frame = {"action": "send_msg", "params": params, "echo": echo}

        try:
            with handle.borrow() as websocket:
                await websocket.send(json.dumps(frame, ensure_ascii=False))
            response = await asyncio.wait_for(
                future, timeout=self.config.request_timeout
            )
        except TimeoutError as exc:
            self._pending.pop(str(echo), None)
            raise OperationInvokeError(
                Error(
                    code=Errs.OPERATION_INVOKE_FAILED,
                    source=self.id,
                    route=_OP_SEND_MSG.op_id,
                    evidence={"reason": "response_timeout"},
                )
            ) from exc
        except Exception as exc:
            self._pending.pop(str(echo), None)
            raise OperationInvokeError(
                Error(
                    code=Errs.OPERATION_INVOKE_FAILED,
                    source=self.id,
                    route=_OP_SEND_MSG.op_id,
                    evidence={
                        "reason": "send_failed",
                        "exception_type": type(exc).__qualname__,
                    },
                )
            ) from exc

        if response.get("status") != "ok" or int(response.get("retcode", -1)) != 0:
            raise OperationInvokeError(
                Error(
                    code=Errs.OPERATION_INVOKE_FAILED,
                    source=self.id,
                    route=_OP_SEND_MSG.op_id,
                    evidence={
                        "reason": "onebot_response_error",
                        "retcode": int(response.get("retcode", -1)),
                    },
                )
            )
        data = response.get("data", {})
        return data if isinstance(data, dict) else {"data": data}

    async def _outbox_pump(self) -> None:
        while True:
            msg = await self.agent.outbox.get()
            if not isinstance(msg, Message):
                continue
            if msg.source.source_id != self.config.source_id:
                continue
            payload = _payload_from_outbound(msg)
            await self.agent.dispatch.invoke(
                _OP_SEND_MSG.op_id,
                payload,
                ctx=self.agent.make_context(message=msg),
            )

    async def _cancel_outbox_task(self) -> None:
        task = self._outbox_task
        if task is None:
            return
        task.cancel()
        with suppress(asyncio.CancelledError):
            await task

    def _authorized(self, websocket: Any) -> bool:
        if self.config.access_token is None:
            return True
        request = getattr(websocket, "request", None)
        headers = getattr(request, "headers", {}) if request is not None else {}
        return headers.get("Authorization") == f"Bearer {self.config.access_token}"


def _extract_text(message: Any) -> tuple[str, int]:
    if isinstance(message, str):
        return message, 0
    if not isinstance(message, list):
        return "", 0
    text_parts: list[str] = []
    non_text = 0
    for segment in message:
        if not isinstance(segment, dict):
            non_text += 1
            continue
        if segment.get("type") == "text":
            data = segment.get("data", {})
            if isinstance(data, dict):
                text_parts.append(str(data.get("text", "")))
        else:
            non_text += 1
    return "".join(text_parts), non_text


def _send_params(payload: dict[str, Any]) -> dict[str, Any]:
    message_type = str(payload.get("message_type", ""))
    if message_type not in {"private", "group"}:
        raise OperationInvokeError(
            Error(
                code=Errs.COMMAND_INVALID_ARGS,
                source=OneBotV11Plugin.id,
                route=_OP_SEND_MSG.op_id,
                evidence={"reason": "invalid_message_type"},
            )
        )
    params: dict[str, Any] = {
        "message_type": message_type,
        "message": str(payload.get("message", "")),
        "auto_escape": bool(payload.get("auto_escape", False)),
    }
    if message_type == "private":
        params["user_id"] = int(payload["user_id"])
    else:
        params["group_id"] = int(payload["group_id"])
    return params


def _payload_from_outbound(msg: Message) -> dict[str, Any]:
    source = msg.source
    channel_id = getattr(source, "channel_id", "")
    if str(channel_id).startswith("group:"):
        return {
            "message_type": "group",
            "group_id": int(str(channel_id).split(":", 1)[1]),
            "message": msg.text,
            "auto_escape": False,
        }
    user_id = getattr(source, "user_id", None) or str(channel_id).split(":", 1)[-1]
    return {
        "message_type": "private",
        "user_id": int(user_id),
        "message": msg.text,
        "auto_escape": False,
    }


__all__ = ["OneBotV11Plugin"]
