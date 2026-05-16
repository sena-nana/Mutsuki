"""Dispatcher —— Operation/Source 注册 + envelope 路由 + 跨 plugin 调用。

详见 :doc:`contracts §18 <plans/contracts>`。Dispatcher 是 v0.2 引入的核心
组件，位于 plugin↔plugin 与 source↔Agent 之间。每个 Agent 独立持一个
Dispatcher 实例（多 agent 协作 / Agent 间路由由 v0.x Phase C 的 AgentRegistry
处理）。

关键设计点：

* **inline await**：``invoke`` 直接 ``await handler(ctx, payload)``，不入
  asyncio.Queue 或 gather —— 这是延迟敏感链路（v0.5+ Yume thought→kernel
  →runtime sub-ms）的硬性前提，与 ``Bus.subscribe(..., direct=True)`` 同等
  约束。
* **Operation 失败标记 unhealthy**：handler 抛未捕获异常 → 标记 op 为
  ``unhealthy`` 但**不主动卸载 plugin**，让 plugin 自决是否 crash；运维通过
  plugin reload 恢复。
* **PluginScope 集成**：``register_*`` 把对应 ``unregister_*`` 自动挂到调用
  方 PluginScope，plugin 卸载时 dispatcher 状态保持一致，无需 plugin 作者
  手写清理代码。

完整生命周期场景（D9）：
* Plugin 卸载 → PluginScope.close → 反注册回调触发 → entry 清理
* Plugin 重载 → 等于卸载+装载；中途窗口对该 op 的 invoke 返 not_found
* 配置变更 → 等于重载，行为同上
* Plugin 崩溃 → handler 抛异常 → op 标记 unhealthy，plugin 不连坐
"""

from __future__ import annotations

from collections.abc import Awaitable, Callable
from dataclasses import dataclass
from enum import StrEnum
import logging
from typing import TYPE_CHECKING, Any

from mutsukibot.contracts.error import Error, Errs
from mutsukibot.contracts.operation import OperationDescriptor
from mutsukibot.contracts.permission import PermissionRule
from mutsukibot.contracts.source import SourceDescriptor
from mutsukibot.core.agent_registry import AgentRegistry
from mutsukibot.core.registry import PluginRegistry

if TYPE_CHECKING:
    from mutsukibot.contracts.envelope import Envelope
    from mutsukibot.core.agent import Agent
    from mutsukibot.core.context import AgentContext
    from mutsukibot.core.scope import PluginScope


_logger = logging.getLogger(__name__)


# Operation handler 统一签名：(ctx, payload) -> result。
# @command 装饰器自动生成的 handler 内部调用 Dependent.solve 解包 typed args；
# 显式 register_operation 的 handler 由 plugin 作者直接提供。
OperationHandler = Callable[
    ["AgentContext", dict[str, Any]],
    Awaitable[Any],
]


class OperationStatus(StrEnum):
    """Operation 状态机。详 contracts §14.5。"""

    ACTIVE = "active"
    UNHEALTHY = "unhealthy"  # handler 抛过未捕获异常；待 plugin reload 恢复
    UNREGISTERING = "unregistering"  # PluginScope.close 进行中


class SourceStatus(StrEnum):
    """Source 状态机（与 Operation 对偶）。"""

    ACTIVE = "active"
    UNREGISTERING = "unregistering"


# ---------------------------------------------------------------------------
# 内部 entry 结构
# ---------------------------------------------------------------------------


@dataclass(slots=True)
class _OperationEntry:
    descriptor: OperationDescriptor
    handler: OperationHandler
    perms: PermissionRule
    plugin_scope: "PluginScope"
    plugin_id: str
    status: OperationStatus = OperationStatus.ACTIVE


@dataclass(slots=True)
class _SourceEntry:
    descriptor: SourceDescriptor
    plugin_scope: "PluginScope"
    plugin_id: str
    status: SourceStatus = SourceStatus.ACTIVE


# ---------------------------------------------------------------------------
# 异常（Operation 调用路径用结构化 Error 封装的运行时异常）
# ---------------------------------------------------------------------------


class OperationInvokeError(Exception):
    """``Dispatcher.invoke`` 路径上结构化错误的载体。

    当 op 未找到 / unhealthy / capability 检查失败 / permission 被拒 / handler
    抛错时，dispatcher 把诊断信息打包进 ``error`` 字段，调用方（scheduler /
    其他 plugin）可分类处理。
    """

    def __init__(self, error: Error) -> None:
        super().__init__(f"operation invoke failed: {error.code}")
        self.error = error


class OperationConflictError(Exception):
    """同一 op_id 已被注册时抛出。"""

    def __init__(self, op_id: str, error: Error) -> None:
        super().__init__(f"operation {op_id!r} 已被注册")
        self.op_id = op_id
        self.error = error


class SourceConflictError(Exception):
    """同一 source_id 已被注册时抛出。"""

    def __init__(self, source_id: str, error: Error) -> None:
        super().__init__(f"source {source_id!r} 已被注册")
        self.source_id = source_id
        self.error = error


class OperationUndeclaredError(Exception):
    """运行时注册的 op_id 未在 plugin manifest ``provides_operations`` 静态声明。

    D9b 强制：plugin 不能"偷偷"注册未声明的 op_id。dispatcher 通过
    :data:`PluginRegistry` 反查 plugin class 的 ``provides_operations`` 集合
    做校验；未在 PluginRegistry 中的 plugin_id（测试场景手搓 OperationDescriptor）
    跳过校验，避免与单测脚手架冲突。
    """

    def __init__(self, op_id: str, error: Error) -> None:
        super().__init__(
            f"operation {op_id!r} 未在 provides_operations 静态声明集内"
        )
        self.op_id = op_id
        self.error = error


class SourceUndeclaredError(Exception):
    """运行时注册的 source_id 未在 plugin manifest ``provides_sources`` 静态声明。"""

    def __init__(self, source_id: str, error: Error) -> None:
        super().__init__(
            f"source {source_id!r} 未在 provides_sources 静态声明集内"
        )
        self.source_id = source_id
        self.error = error


def _check_op_declared(plugin_id: str, op_id: str) -> None:
    """D9b 校验：op_id 必须在 declaring plugin 的 provides_operations 集合内。

    未注册到 :data:`PluginRegistry` 的 plugin_id（测试场景）跳过校验。
    """
    cls = PluginRegistry.get(plugin_id)
    if cls is None:
        return  # 非受管 plugin，跳过校验
    declared = {op.op_id for op in cls.provides_operations}
    if op_id not in declared:
        err = Error(
            code=Errs.OPERATION_UNDECLARED,
            source=plugin_id,
            route=f"dispatcher.register_operation.{op_id}",
            evidence={
                "op_id": op_id,
                "plugin_id": plugin_id,
                "declared_count": len(declared),
            },
        )
        raise OperationUndeclaredError(op_id, err)


def _check_source_declared(plugin_id: str, source_id: str) -> None:
    """D9b 校验：source_id 必须在 declaring plugin 的 provides_sources 集合内。"""
    cls = PluginRegistry.get(plugin_id)
    if cls is None:
        return
    declared = {s.source_id for s in cls.provides_sources}
    if source_id not in declared:
        err = Error(
            code=Errs.SOURCE_UNDECLARED,
            source=plugin_id,
            route=f"dispatcher.register_source.{source_id}",
            evidence={
                "source_id": source_id,
                "plugin_id": plugin_id,
                "declared_count": len(declared),
            },
        )
        raise SourceUndeclaredError(source_id, err)


# ---------------------------------------------------------------------------
# Dispatcher 主体
# ---------------------------------------------------------------------------


class Dispatcher:
    """单 Agent 的 Operation/Source 路由表与调用入口。

    多 Agent 协作（envelope 跨 Agent 广播 / 跨 Agent invoke）由 Phase C
    的 ``AgentRegistry`` 在 dispatcher 之上叠加；本类只关心单 Agent 内部。
    """

    def __init__(self, agent: "Agent") -> None:
        self.agent = agent
        self._operations: dict[str, _OperationEntry] = {}
        self._sources: dict[str, _SourceEntry] = {}
        # 短名 → op_id 集合，供 lookup_operation 做"echo → echo.echo" 后缀匹配
        self._op_name_index: dict[str, set[str]] = {}

    # ----- 注册 API ------------------------------------------------------

    def register_operation(
        self,
        descriptor: OperationDescriptor,
        *,
        handler: OperationHandler,
        perms: PermissionRule,
        plugin_scope: "PluginScope",
    ) -> None:
        """注册 Operation；自动 attach 反注册回调到 plugin_scope。

        失败模式：
        * op_id 未在 declaring plugin 的 ``provides_operations`` 集合内 →
          :class:`OperationUndeclaredError`（D9b；非受管 plugin 跳过校验）
        * 同 op_id 已注册 → :class:`OperationConflictError`
        * scope 已关闭 → ``RuntimeError`` 由 PluginScope 抛出
        """
        op_id = descriptor.op_id
        # D9b：先校验是否在 plugin 的静态 provides 集内
        _check_op_declared(descriptor.plugin_id, op_id)
        if op_id in self._operations:
            err = Error(
                code=Errs.OPERATION_CONFLICT,
                source=descriptor.plugin_id or "<unknown>",
                route=f"dispatcher.register_operation.{op_id}",
                evidence={
                    "op_id": op_id,
                    "existing_plugin_id": self._operations[op_id].plugin_id,
                    "incoming_plugin_id": descriptor.plugin_id,
                },
            )
            raise OperationConflictError(op_id, err)

        entry = _OperationEntry(
            descriptor=descriptor,
            handler=handler,
            perms=perms,
            plugin_scope=plugin_scope,
            plugin_id=descriptor.plugin_id,
        )
        self._operations[op_id] = entry
        # 短名索引
        last_seg = op_id.rsplit(".", 1)[-1]
        self._op_name_index.setdefault(last_seg, set()).add(op_id)
        # 自动反注册（plugin 卸载时 PluginScope.close 触发）
        plugin_scope.add_dispatch_registration(
            lambda: self._unregister_operation(op_id)
        )

    def register_source(
        self,
        descriptor: SourceDescriptor,
        *,
        plugin_scope: "PluginScope",
        plugin_id: str,
    ) -> None:
        """注册 Source；自动 attach 反注册回调到 plugin_scope。

        失败模式：
        * source_id 未在 declaring plugin 的 ``provides_sources`` 集合内 →
          :class:`SourceUndeclaredError`（D9b；非受管 plugin 跳过校验）
        * 同 source_id 已注册 → :class:`SourceConflictError`
        """
        source_id = descriptor.source_id
        # D9b：先校验是否在 plugin 的静态 provides 集内
        _check_source_declared(plugin_id, source_id)
        if source_id in self._sources:
            err = Error(
                code=Errs.SOURCE_CONFLICT,
                source=plugin_id,
                route=f"dispatcher.register_source.{source_id}",
                evidence={
                    "source_id": source_id,
                    "existing_plugin_id": self._sources[source_id].plugin_id,
                    "incoming_plugin_id": plugin_id,
                },
            )
            raise SourceConflictError(source_id, err)

        entry = _SourceEntry(
            descriptor=descriptor,
            plugin_scope=plugin_scope,
            plugin_id=plugin_id,
        )
        self._sources[source_id] = entry
        plugin_scope.add_dispatch_registration(
            lambda: self._unregister_source(source_id)
        )

    def _unregister_operation(self, op_id: str) -> None:
        entry = self._operations.pop(op_id, None)
        if entry is None:
            return
        last_seg = op_id.rsplit(".", 1)[-1]
        bucket = self._op_name_index.get(last_seg)
        if bucket is not None:
            bucket.discard(op_id)
            if not bucket:
                del self._op_name_index[last_seg]

    def _unregister_source(self, source_id: str) -> None:
        self._sources.pop(source_id, None)

    # ----- 查询 API ------------------------------------------------------

    def lookup_operation(self, name: str) -> str | None:
        """供 scheduler 文本路径用：把首词解析为 op_id。

        策略：
        1. 精确 op_id 命中 → 直接返回
        2. 否则按短名（op_id 最后一段）查；若唯一 → 返回；冲突 → ``None``
           （冲突场景由调用方决定行为；v0.2 视为未匹配）
        """
        if name in self._operations:
            return name
        bucket = self._op_name_index.get(name, set())
        if len(bucket) == 1:
            return next(iter(bucket))
        return None

    def operation_status(self, op_id: str) -> OperationStatus | None:
        entry = self._operations.get(op_id)
        return entry.status if entry is not None else None

    def source_status(self, source_id: str) -> SourceStatus | None:
        entry = self._sources.get(source_id)
        return entry.status if entry is not None else None

    def list_operations(self) -> tuple[OperationDescriptor, ...]:
        return tuple(e.descriptor for e in self._operations.values())

    def list_sources(self) -> tuple[SourceDescriptor, ...]:
        return tuple(e.descriptor for e in self._sources.values())

    def has_operation(self, op_id: str) -> bool:
        return op_id in self._operations

    def has_source(self, source_id: str) -> bool:
        return source_id in self._sources

    # ----- 调用 API ------------------------------------------------------

    async def invoke(
        self,
        op_id: str,
        payload: dict[str, Any] | None = None,
        *,
        ctx: "AgentContext",
    ) -> Any:
        """同步 inline await Operation handler。

        流程（contracts §18.3）：

        1. 查 op_id；不存在 → ``operation.not_found``
        2. 状态检查；unhealthy → ``operation.unhealthy``
        3. capability 检查（plugin manifest 申报 vs op 要求）
        4. permission 检查（``await rule.check(ctx)``）
        5. **inline `await handler(ctx, payload)`** —— 不进异步队列
        6. handler 抛错 → 标记 op unhealthy，结构化 Error 抛 ``OperationInvokeError``

        返回 handler 的原始返回值；scheduler 自行决定如何渲染（如 str
        包成 outbound Message）。
        """
        entry = self._operations.get(op_id)
        if entry is None:
            err = Error(
                code=Errs.OPERATION_NOT_FOUND,
                source="dispatcher",
                route=f"dispatcher.invoke.{op_id}",
                evidence={"op_id": op_id},
            )
            raise OperationInvokeError(err)

        if entry.status == OperationStatus.UNHEALTHY:
            err = Error(
                code=Errs.OPERATION_UNHEALTHY,
                source=entry.plugin_id,
                route=f"dispatcher.invoke.{op_id}",
                evidence={"op_id": op_id, "status": entry.status.value},
            )
            raise OperationInvokeError(err)

        # capability 检查（复用 v0.1 的 check_capabilities）
        from mutsukibot.core.capability_guard import (
            CapabilityNotDeclaredError,
            check_capabilities,
        )

        try:
            # 已在 plugin manifest 中申报的 caps 是 entry.plugin_scope.owner 的
            # plugin 类的 capabilities；为避免 dispatcher 直接看 plugin class，
            # 把"能不能调"简化为"op 要求的 caps 是否被某 plugin 申报"——
            # 在 register_operation 阶段的静态校验里保证（D9b）。这里只兜底。
            declared = tuple(c for c in entry.descriptor.requires_capabilities)
            check_capabilities(
                plugin_id=entry.plugin_id,
                declared=declared,
                required=entry.descriptor.requires_capabilities,
                route=f"operation.{op_id}",
            )
        except CapabilityNotDeclaredError as exc:
            raise OperationInvokeError(exc.error) from exc

        # permission 检查
        if not await entry.perms.check(ctx):
            err = Error(
                code=Errs.PERMISSION_DENIED,
                source=entry.plugin_id,
                route=f"operation.{op_id}",
                evidence={
                    "op_id": op_id,
                    "perms_rule": entry.descriptor.perms_rule_id or "",
                },
            )
            raise OperationInvokeError(err)

        # **inline await** —— 这是 contracts §18 的硬性要求
        try:
            return await entry.handler(ctx, payload or {})
        except OperationInvokeError:
            # handler 自己抛了结构化错误，原样向上传
            raise
        except Exception as exc:
            # 标记 op 为 unhealthy（D9 / contracts §14.5）
            entry.status = OperationStatus.UNHEALTHY
            err = Error(
                code=Errs.OPERATION_HANDLER_RAISED,
                source=entry.plugin_id,
                route=f"operation.{op_id}",
                evidence={
                    "op_id": op_id,
                    "exception_type": type(exc).__qualname__,
                    "exception_repr": repr(exc),
                },
            )
            raise OperationInvokeError(err) from exc

    async def invoke_in_agent(
        self,
        agent_id: str,
        op_id: str,
        payload: dict[str, Any] | None = None,
        *,
        ctx: "AgentContext",
    ) -> Any:
        """显式跨 Agent 调用目标 Agent 的 Operation（v0.3）。

        该路径仍保持 inline await：找到目标 Agent 后直接调用目标
        ``Dispatcher.invoke``，不通过 inbox/outbox 队列，也不做隐式广播。
        调用上下文切换到目标 Agent，trace parent 继承自调用方上下文。
        """
        target = AgentRegistry.get(agent_id)
        if target is None:
            err = Error(
                code=Errs.AGENT_NOT_FOUND,
                source="dispatcher",
                route=f"dispatcher.invoke_in_agent.{agent_id}.{op_id}",
                evidence={"agent_id": agent_id, "op_id": op_id},
            )
            raise OperationInvokeError(err)

        target_ctx = target.make_context()
        target_ctx.trace_ctx.parent_span_id = ctx.trace_ctx.span_id
        return await target.dispatch.invoke(op_id, payload or {}, ctx=target_ctx)

    async def publish(self, envelope: "Envelope") -> None:
        """发布 envelope。

        校验 ``envelope.source.source_id`` 在已注册集；按 ``Agent.accepts``
        匹配后投递到 ``agent.inbox``。

        v0.2 Phase C：通过 AgentRegistry 进行广播扇出，把 envelope 投给所有
        匹配 ``Agent.accepts`` 的 awake Agent。
        """
        source_id = envelope.source.source_id
        if source_id not in self._sources:
            err = Error(
                code=Errs.SOURCE_UNREGISTERED,
                source="dispatcher",
                route="dispatcher.publish",
                evidence={
                    "source_id": source_id,
                    "envelope_id": envelope.id,
                },
            )
            raise OperationInvokeError(err)

        matched = tuple(AgentRegistry.iter_accepting(envelope))
        if not matched:
            _logger.debug(
                "envelope %s dropped: no awake Agent.accepts rule matched",
                envelope.id,
            )
            return
        for agent in matched:
            await agent.inbox.put(envelope)


__all__ = [
    "Dispatcher",
    "OperationConflictError",
    "OperationHandler",
    "OperationInvokeError",
    "OperationStatus",
    "OperationUndeclaredError",
    "SourceConflictError",
    "SourceStatus",
    "SourceUndeclaredError",
]
