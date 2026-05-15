"""Todo reference plugin —— v0.2 工具型 endpoint 的标准范例。

这是一个 **tool kind** 的 reference plugin：它通过 dispatcher 注册三个
Operations（``todo:default.create`` / ``.list`` / ``.complete``），让其他
plugin 通过 ``ctx.dispatch.invoke("todo:default.create", {...})`` 跨 endpoint
调用。

**Hard rule #14 演示**：内存存储 dict 通过 ``Handle`` attach 到 PluginScope
（参见 ``_register_storage_handle``），plugin 卸载时 finalizer 清空数据，
保证不残留状态。这与生产 plugin 持有 socket / SDK client 的模式同构。

**D9b 演示**：``provides_operations`` 在 manifest 静态声明，dispatcher 在
register 时校验；如果某 op 未声明就被 register，会被立即拒。
"""

from __future__ import annotations

from typing import Any, ClassVar

import msgspec

from mutsukibot import Capability, Caps, Plugin
from mutsukibot.contracts import OperationDescriptor, Perms, RefId
from mutsukibot.contracts.error import Error, Errs
from mutsukibot.core.dispatcher import OperationInvokeError
from mutsukibot.core.handle import make_stub_handle


class _TodoConfig(msgspec.Struct, kw_only=True):
    source_id: str = "todo:default"


# 静态声明三个 op；运行时 on_load 用同一对象注册（D9b 静态 == 运行时一致性）
_OP_CREATE = OperationDescriptor(
    op_id="todo:default.create",
    name="todo_create",
    description="新建一个 todo item，返回 item_id。",
    plugin_id="mutsukibot-todo",
    requires_capabilities=(Caps.PERSIST,),
    parameters_schema={
        "type": "object",
        "properties": {"text": {"type": "string"}},
        "required": ["text"],
    },
    return_schema={"type": "string"},
)
_OP_LIST = OperationDescriptor(
    op_id="todo:default.list",
    name="todo_list",
    description="列出所有 todo items。",
    plugin_id="mutsukibot-todo",
    parameters_schema={"type": "object", "properties": {}},
    return_schema={"type": "array"},
)
_OP_COMPLETE = OperationDescriptor(
    op_id="todo:default.complete",
    name="todo_complete",
    description="标记某 item 为已完成。",
    plugin_id="mutsukibot-todo",
    parameters_schema={
        "type": "object",
        "properties": {"item_id": {"type": "string"}},
        "required": ["item_id"],
    },
)


class TodoPlugin(Plugin[_TodoConfig]):
    """In-memory todo store 的 reference plugin。"""

    id: ClassVar[str] = "mutsukibot-todo"
    version: ClassVar[str] = "0.2.0"
    capabilities: ClassVar[list[Capability]] = [
        Capability(name=Caps.PERSIST),
    ]
    provides_operations: ClassVar[tuple[OperationDescriptor, ...]] = (
        _OP_CREATE,
        _OP_LIST,
        _OP_COMPLETE,
    )
    Config = _TodoConfig

    async def on_load(self) -> None:
        # 状态外置到 Handle —— hard rule #14 演示。生产 plugin 持有 socket /
        # SDK client / KV cache 的模式与之同构（详见 plans/contracts.md §11）。
        # `make_stub_handle` 用于无真实后端的可观测假 handle。
        self._items: dict[str, dict[str, Any]] = {}
        self._handle = make_stub_handle(
            ref_id=RefId("todo:default.storage"),
            kind="mutsukibot.todo.storage",
            schema_id_target="mutsukibot.todo.dict_storage",
            schema_version_target="1.0.0",
            target=self._items,
        )
        self._handle.attach_to(self.scope)

        # 注册三个 Operation；handler 内 await 直接处理同步字典，无 I/O。
        register = self.agent.dispatch.register_operation
        register(
            _OP_CREATE,
            handler=self._handle_create,
            perms=Perms.PUBLIC.to_rule(),
            plugin_scope=self.scope,
        )
        register(
            _OP_LIST,
            handler=self._handle_list,
            perms=Perms.PUBLIC.to_rule(),
            plugin_scope=self.scope,
        )
        register(
            _OP_COMPLETE,
            handler=self._handle_complete,
            perms=Perms.PUBLIC.to_rule(),
            plugin_scope=self.scope,
        )

    async def _handle_create(self, _ctx, payload: dict[str, Any]) -> str:
        text = payload.get("text", "").strip()
        if not text:
            raise OperationInvokeError(
                Error(
                    code=Errs.COMMAND_INVALID_ARGS,
                    source=self.id,
                    route="todo:default.create",
                    evidence={"reason": "empty_text"},
                )
            )
        item_id = self.agent.id_gen.next("todo")
        self._items[item_id] = {"text": text, "done": False}
        return item_id

    async def _handle_list(self, _ctx, _payload: dict[str, Any]) -> list[dict[str, Any]]:
        return [{"id": k, **v} for k, v in self._items.items()]

    async def _handle_complete(self, _ctx, payload: dict[str, Any]) -> bool:
        item_id = payload.get("item_id", "")
        item = self._items.get(item_id)
        if item is None:
            return False
        item["done"] = True
        return True


__all__ = ["TodoPlugin"]
