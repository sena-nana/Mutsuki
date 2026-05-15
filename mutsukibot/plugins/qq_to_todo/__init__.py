"""qq_to_todo bridge plugin —— v0.2 跨 endpoint 协作的标准范例。

演示 [contracts.md §17 ScopeRule](../../../plans/contracts.md) 与 §18
Dispatcher 的核心组合：

* ``consumes`` 声明本 plugin 关注 IM 文本消息（``Scopes.IM_TEXT``）
* scheduler 二次分发到本 plugin 的 ``on_envelope``
* handler 解析消息文本，凡以 ``"todo "`` 开头的内容 → 调 ``ctx.dispatch.invoke
  ("todo:default.create", ...)``，把任务写到 Todo plugin
* ``requires_operations`` 声明对 ``todo:default.create`` 的硬依赖 ——
  PluginLoader 会用它做 DAG 排序，保证 todo plugin 先于本 plugin 装载

这就是用户场景"QQ 对话操作 Todo 数据"的最小可运行范式：跨"adapter"调用
通过 ``ctx.dispatch.invoke(endpoint_id.op, payload)`` 字面表达，目标 endpoint
名在调用现场可见，满足 [设计原则 #4 易识别性](../../../plans/contracts.md)。
"""

from __future__ import annotations

from typing import ClassVar

import msgspec

from mutsukibot import Capability, Caps, Plugin
from mutsukibot.contracts import (
    BySchema,
    BySourceKind,
    Envelope,
    Message,
    OperationDep,
    ScopeRule,
    SourceKinds,
)


class _BridgeConfig(msgspec.Struct, kw_only=True):
    target_op: str = "todo:default.create"
    prefix: str = "todo "


class QqToTodoPlugin(Plugin[_BridgeConfig]):
    """把 IM 文本中以 ``todo `` 开头的内容转发到 todo endpoint。"""

    id: ClassVar[str] = "mutsukibot-qq-to-todo"
    version: ClassVar[str] = "0.2.0"
    capabilities: ClassVar[list[Capability]] = [
        Capability(name=Caps.READ_MESSAGE),
    ]
    # 静态声明依赖 todo:default.create —— PluginLoader DAG 据此把 todo plugin
    # 排在本 plugin 之前装载（D9b）。
    requires_operations: ClassVar[tuple[OperationDep, ...]] = (
        OperationDep(op_id="todo:default.create"),
    )
    # consumes：scheduler 仅当 envelope 是 IM 文本消息时调用 on_envelope。
    consumes: ClassVar[tuple[ScopeRule, ...]] = (
        BySchema("mutsukibot.message") & BySourceKind(SourceKinds.IM),
    )
    Config = _BridgeConfig

    async def on_envelope(self, envelope: Envelope) -> None:
        if not isinstance(envelope, Message):
            return
        text = envelope.text.strip()
        prefix = self.config.prefix
        if not text.startswith(prefix):
            return
        body = text[len(prefix):].strip()
        if not body:
            return
        # 跨 endpoint 调用 —— target 字面量"todo:default.create"清晰可见
        ctx = self.agent.make_context(message=envelope)
        item_id = await self.agent.dispatch.invoke(
            self.config.target_op,
            {"text": body},
            ctx=ctx,
        )
        # 通过 bus 发一条审计事件（observability 可订阅）
        await self.agent.bus.publish(
            "qq_to_todo.created",
            {"item_id": item_id, "source": envelope.source.source_id},
        )


__all__ = ["QqToTodoPlugin"]
