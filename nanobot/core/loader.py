"""插件加载器：entry_points 发现 + DAG 拓扑装载/卸载。

发现流程：

1. 枚举 ``importlib.metadata.entry_points(group="nanobot.plugins")``。
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

from nanobot.contracts.error import Error, Errs
from nanobot.core.agent import Agent
from nanobot.core.plugin import Plugin
from nanobot.core.registry import PluginRegistry
from nanobot.core.scope import PluginScope


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


class PluginNotFoundError(KeyError):
    pass


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
        entry_point_group: str = "nanobot.plugins",
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
        """按拓扑顺序把插件实例化进 ``agent``，再 await ``on_load``。"""
        configs = configs or {}
        by_id: dict[str, type[Plugin]] = {cls.id: cls for cls in plugin_classes}

        deps_map = {
            pid: tuple(d.plugin_id for d in cls.requires_plugins)
            for pid, cls in by_id.items()
        }
        order = _toposort(deps_map)

        for pid in order:
            cls = by_id[pid]
            cfg = configs.get(pid) or cls.Config()
            scope = PluginScope(owner=pid)
            instance = cls(
                agent=agent,
                config=cfg,
                scope=scope,
                services=agent.services,
                bus=agent.bus,
            )
            agent.attach_plugin(instance, scope)
            await instance.on_load()

    async def unload_from(self, agent: Agent) -> None:
        """按加载反序运行 ``on_unload`` 然后 ``scope.close()``。"""
        while agent.plugins:
            entry = agent.plugins.pop()
            agent.detach_plugin(entry.plugin)
            try:
                await entry.plugin.on_unload()
            finally:
                await entry.scope.close()
                PluginRegistry.unregister(entry.plugin.id)
                # 卸载的是 *实例*，不是类；把类重新登记回去。
                PluginRegistry.register(entry.plugin.id, type(entry.plugin))


__all__ = [
    "PluginCycleError",
    "PluginDependencyMissingError",
    "PluginLoader",
    "PluginNotFoundError",
]
