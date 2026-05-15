"""插件加载器：entry_points 发现 + DAG 拓扑装载/卸载。

发现流程：

1. 枚举 ``importlib.metadata.entry_points(group="mutsukibot.plugins")``。
2. 可选的显式白名单（传给 :class:`PluginLoader`）；设了就只加载白名单内的 id。
3. 把每个 entry point 解析成 :class:`Plugin` 子类 —— 这会触发
   :class:`PluginMeta` 校验与注册。
4. 按 ``requires_plugins`` 做拓扑排序；存在环则拒绝。
5. 对每个插件：构造 ``Config()`` → 新建 :class:`PluginScope` → 实例化插件
   → await ``on_load``。

卸载按反序：``on_unload`` → ``scope.close()``。scope 关闭时若发现泄漏会以
:class:`HandleLeakError` 形式抛出。
"""

from __future__ import annotations

from collections.abc import Iterable
import graphlib
import importlib.metadata

import msgspec

from mutsukibot.contracts.error import Error, Errs
from mutsukibot.core.agent import Agent
from mutsukibot.core.plugin import Plugin
from mutsukibot.core.scope import PluginScope


class PluginCycleError(Exception):
    def __init__(self, cycle: list[str], err: Error) -> None:
        super().__init__(f"插件 DAG 存在环: {' -> '.join(cycle)}")
        self.cycle = cycle
        self.error = err


class PluginDependencyMissingError(Exception):
    """声明了 ``requires_plugins`` 但依赖未在装载列表中时抛出。"""

    def __init__(self, missing: list[tuple[str, str]], err: Error) -> None:
        pretty = ", ".join(f"{src}->{dep}" for src, dep in missing)
        super().__init__(f"插件依赖缺失: {pretty}")
        self.missing = missing
        self.error = err


class PluginLoadFailedError(Exception):
    """插件 ``on_load`` 抛错（或实例化失败）；已加载的插件已经被反向回滚。"""

    def __init__(self, plugin_id: str, err: Error) -> None:
        super().__init__(f"插件 {plugin_id!r} 加载失败")
        self.plugin_id = plugin_id
        self.error = err


class PluginNotFoundError(KeyError):
    pass


class OperationProvisionConflictError(Exception):
    """两个不同 plugin 在 ``provides_operations`` 静态声明同一 op_id（D9b）。"""

    def __init__(self, op_id: str, owners: tuple[str, str], err: Error) -> None:
        super().__init__(
            f"op_id {op_id!r} 被 {owners[0]!r} 与 {owners[1]!r} 同时声明"
        )
        self.op_id = op_id
        self.owners = owners
        self.error = err


class SourceProvisionConflictError(Exception):
    """两个不同 plugin 在 ``provides_sources`` 静态声明同一 source_id（D9b）。"""

    def __init__(self, source_id: str, owners: tuple[str, str], err: Error) -> None:
        super().__init__(
            f"source_id {source_id!r} 被 {owners[0]!r} 与 {owners[1]!r} 同时声明"
        )
        self.source_id = source_id
        self.owners = owners
        self.error = err


def _build_provision_indexes(
    by_id: dict[str, "type[Plugin]"],
) -> tuple[dict[str, str], dict[str, str]]:
    """返回 (op_id → providing_plugin_id, source_id → providing_plugin_id)。

    冲突立即抛 :class:`OperationProvisionConflictError` /
    :class:`SourceProvisionConflictError`（D9b 静态闸口，与 dispatcher 运行时
    校验配对）。
    """
    op_owner: dict[str, str] = {}
    src_owner: dict[str, str] = {}
    for pid, cls in by_id.items():
        for op in cls.provides_operations:
            existing = op_owner.get(op.op_id)
            if existing is not None and existing != pid:
                err = Error(
                    code=Errs.OPERATION_CONFLICT,
                    source="core.loader",
                    route="plugin.discover",
                    evidence={
                        "op_id": op.op_id,
                        "owner_a": existing,
                        "owner_b": pid,
                    },
                )
                raise OperationProvisionConflictError(
                    op.op_id, (existing, pid), err
                )
            op_owner[op.op_id] = pid
        for src in cls.provides_sources:
            existing_s = src_owner.get(src.source_id)
            if existing_s is not None and existing_s != pid:
                err = Error(
                    code=Errs.SOURCE_CONFLICT,
                    source="core.loader",
                    route="plugin.discover",
                    evidence={
                        "source_id": src.source_id,
                        "owner_a": existing_s,
                        "owner_b": pid,
                    },
                )
                raise SourceProvisionConflictError(
                    src.source_id, (existing_s, pid), err
                )
            src_owner[src.source_id] = pid
    return op_owner, src_owner


def _resolve_provision_deps(
    by_id: dict[str, "type[Plugin]"],
    op_owner: dict[str, str],
    src_owner: dict[str, str],
) -> dict[str, set[str]]:
    """把 ``requires_operations`` / ``requires_sources`` 翻译为 plugin-level 依赖。

    返回 plugin_id → set[依赖 plugin_id]。未在 op_owner / src_owner 中找到
    的 requires 视为缺失依赖，抛 :class:`PluginDependencyMissingError`，
    与 ``requires_plugins`` 缺失同样的 fail-loud 语义。
    """
    extra_deps: dict[str, set[str]] = {pid: set() for pid in by_id}
    missing: list[tuple[str, str]] = []
    for pid, cls in by_id.items():
        for op_dep in cls.requires_operations:
            owner = op_owner.get(op_dep.op_id)
            if owner is None:
                missing.append((pid, f"op:{op_dep.op_id}"))
                continue
            if owner != pid:
                extra_deps[pid].add(owner)
        for src_dep in cls.requires_sources:
            owner = src_owner.get(src_dep.source_id)
            if owner is None:
                missing.append((pid, f"source:{src_dep.source_id}"))
                continue
            if owner != pid:
                extra_deps[pid].add(owner)
    if missing:
        err = Error(
            code=Errs.PLUGIN_DEPENDENCY_MISSING,
            source="core.loader",
            route="plugin.dag",
            evidence={"missing": ",".join(f"{s}->{d}" for s, d in missing)},
        )
        raise PluginDependencyMissingError(missing, err)
    return extra_deps


def _toposort(items: dict[str, tuple[str, ...]]) -> list[str]:
    """委托给 :class:`graphlib.TopologicalSorter`。

    ``items[node]`` 是 ``node`` 依赖的节点 —— 与 ``TopologicalSorter`` 期望
    的「predecessors」语义一致。任何依赖不在 ``items`` 里的节点都视为缺失，
    立即抛 :class:`PluginDependencyMissingError`：错误延迟到运行时
    ``ServiceNotFound`` 比早 fail 难调试得多。
    """
    missing: list[tuple[str, str]] = [
        (node, dep)
        for node, deps in items.items()
        for dep in deps
        if dep not in items
    ]
    if missing:
        err = Error(
            code=Errs.PLUGIN_DEPENDENCY_MISSING,
            source="core.loader",
            route="plugin.dag",
            evidence={"missing": ",".join(f"{s}->{d}" for s, d in missing)},
        )
        raise PluginDependencyMissingError(missing, err)
    sorter = graphlib.TopologicalSorter(items)
    try:
        return list(sorter.static_order())
    except graphlib.CycleError as exc:
        cycle: list[str] = (
            [str(n) for n in exc.args[1]] if len(exc.args) > 1 else []
        )
        err = Error(
            code=Errs.PLUGIN_CYCLE,
            source="core.loader",
            route="plugin.dag",
            evidence={"remaining": ",".join(cycle)},
        )
        raise PluginCycleError(cycle, err) from exc


class PluginLoader:
    def __init__(
        self,
        *,
        entry_point_group: str = "mutsukibot.plugins",
        allow: Iterable[str] | None = None,
    ) -> None:
        self._group = entry_point_group
        self._allow: set[str] | None = set(allow) if allow is not None else None

    def discover(self) -> list[type[Plugin]]:
        """解析 entry_points，返回发现的 Plugin 类列表。"""
        eps = importlib.metadata.entry_points(group=self._group)
        discovered: list[type[Plugin]] = []
        for ep in eps:
            cls = ep.load()
            if not (isinstance(cls, type) and issubclass(cls, Plugin)):
                raise TypeError(
                    f"entry point {ep.name!r} 解析结果不是 Plugin 子类"
                )
            if self._allow is not None and cls.id not in self._allow:
                continue
            discovered.append(cls)
        return discovered

    async def load_into(
        self,
        agent: Agent,
        plugin_classes: Iterable[type[Plugin]],
        configs: dict[str, msgspec.Struct] | None = None,
    ) -> None:
        """按拓扑顺序把插件实例化进 ``agent``，再 await ``on_load``。

        某个插件 ``on_load`` 抛错时，已加载的插件会按反向顺序被自动卸载
        （``on_unload`` + ``scope.close``），保证 agent 不会停留在半加载
        状态。最终抛出 :class:`PluginLoadFailedError`，``cause`` 链指向
        原始异常以便排障。
        """
        configs = configs or {}
        by_id: dict[str, type[Plugin]] = {cls.id: cls for cls in plugin_classes}

        # D9b：先建立 op_id / source_id → providing_plugin_id 反向索引
        # （冲突立刻 fail），再把 requires_operations / requires_sources
        # 翻译为 plugin-level 依赖加入 DAG，与 requires_plugins 同一拓扑。
        op_owner, src_owner = _build_provision_indexes(by_id)
        provision_deps = _resolve_provision_deps(by_id, op_owner, src_owner)

        deps_map: dict[str, tuple[str, ...]] = {}
        for pid, cls in by_id.items():
            combined: set[str] = {d.plugin_id for d in cls.requires_plugins}
            combined.update(provision_deps.get(pid, set()))
            deps_map[pid] = tuple(sorted(combined))
        order = _toposort(deps_map)

        loaded_so_far: list[tuple[Plugin, PluginScope]] = []
        for pid in order:
            cls = by_id[pid]
            cfg = configs.get(pid) or cls.Config()
            scope = PluginScope(owner=pid)
            instance: Plugin | None = None
            try:
                instance = cls(
                    agent=agent,
                    config=cfg,
                    scope=scope,
                    services=agent.services,
                    bus=agent.bus,
                )
                # on_load 先跑：如果它抛错，instance 还没进 agent.plugins /
                # _command_index，只需要关闭 scope 释放可能已注册的资源。
                await instance.on_load()
                # on_load 成功才登记，避免半加载状态污染查询路径。
                agent.attach_plugin(instance, scope)
            except Exception as exc:
                # 当前失败插件：scope 可能已有部分资源
                try:
                    await scope.close()
                except Exception:
                    pass
                # 反向卸载之前成功加载的插件
                await self._rollback(agent, loaded_so_far)
                err = Error(
                    code=Errs.PLUGIN_LOAD_FAILED,
                    source="core.loader",
                    route=f"plugin.load.{pid}",
                    evidence={
                        "plugin_id": pid,
                        "rolled_back": len(loaded_so_far),
                        "exception_type": type(exc).__qualname__,
                        "exception_repr": repr(exc),
                    },
                )
                raise PluginLoadFailedError(pid, err) from exc
            loaded_so_far.append((instance, scope))

    async def _rollback(
        self,
        agent: Agent,
        loaded: list[tuple[Plugin, PluginScope]],
    ) -> None:
        """反向卸载已成功加载的插件。回滚阶段的次生异常吞掉以便聚合上报原因。"""
        for inst, scope in reversed(loaded):
            agent.detach_plugin(inst)
            agent.plugins[:] = [p for p in agent.plugins if p.plugin is not inst]
            try:
                await inst.on_unload()
            except Exception:
                pass
            try:
                await scope.close()
            except Exception:
                pass

    async def unload_from(self, agent: Agent) -> None:
        """按加载反序运行 ``on_unload`` 然后 ``scope.close()``。

        注意：``PluginRegistry`` 存的是 Plugin **类**（由 ``PluginMeta`` 在
        类定义阶段注册），卸载实例不应触碰类注册表。原先这里的
        ``unregister`` + ``register`` 是 no-op 但中间窗口为空，多 Agent
        共享同一插件类时有竞态，故移除。
        """
        while agent.plugins:
            entry = agent.plugins.pop()
            agent.detach_plugin(entry.plugin)
            try:
                await entry.plugin.on_unload()
            finally:
                await entry.scope.close()


__all__ = [
    "OperationProvisionConflictError",
    "PluginCycleError",
    "PluginDependencyMissingError",
    "PluginLoadFailedError",
    "PluginLoader",
    "PluginNotFoundError",
    "SourceProvisionConflictError",
]
