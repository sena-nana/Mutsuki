# 插件 DAG 加载

## 这是什么

`PluginLoader` 负责发现、按依赖拓扑排序、校验配置、装载、卸载插件。装载顺序由 `requires_plugins`、`requires_operations`、`requires_sources` 共同形成的 DAG 决定，存在缺失或环则拒绝启动。

代码：[mutsukibot/core/loader.py](../../mutsukibot/core/loader.py)。

## 解决什么问题

插件之间通过 service / dispatcher / bus 通信，但服务或 Operation **实例化**仍有顺序依赖：用户搜索插件依赖 HTTP 客户端插件，HTTP 必须先 `on_load` 注册服务或 Operation，搜索插件 `on_load` 时才解析得到。靠人工排顺序不可持续 —— 一旦插件多了，加载 / 卸载顺序就成了隐含约定。DAG 拓扑排序把它形式化。

## 怎么工作

### 发现：entry_points

[loader.py:77-90](../../mutsukibot/core/loader.py#L77-L90)：

```python
def discover(self) -> list[type[Plugin]]:
    eps = importlib.metadata.entry_points(group=self._group)
    discovered: list[type[Plugin]] = []
    for ep in eps:
        cls = ep.load()
        if not (isinstance(cls, type) and issubclass(cls, Plugin)):
            raise TypeError(...)
        if self._allow is not None and cls.id not in self._allow:
            continue
        discovered.append(cls)
    return discovered
```

约定：`pyproject.toml` 里声明 `[project.entry-points."mutsukibot.plugins"]`：

```toml
[project.entry-points."mutsukibot.plugins"]
echo = "mutsukibot.plugins.echo:EchoPlugin"
```

允许传 `allow={...}` 白名单只装一部分，便于测试。

### 拓扑排序：graphlib

[loader.py](../../mutsukibot/core/loader.py)：

```python
def _toposort(items: dict[str, tuple[str, ...]]) -> list[str]:
    missing = [
        (node, dep)
        for node, deps in items.items()
        for dep in deps
        if dep not in items
    ]
    if missing:
        raise PluginDependencyMissingError(...)
    sorter = graphlib.TopologicalSorter(items)
    try:
        return list(sorter.static_order())
    except graphlib.CycleError as exc:
        cycle = [str(n) for n in exc.args[1]] if len(exc.args) > 1 else []
        err = Error(
            code=Errs.PLUGIN_CYCLE,
            source="core.loader",
            route="plugin.dag",
            evidence={"remaining": ",".join(cycle)},
        )
        raise PluginCycleError(cycle, err) from exc
```

要点：

- **缺失依赖 fail-loud**——`requires_plugins=[PluginDep(plugin_id="some-other")]` 但 `some-other` 没在本次装载列表里，loader 会抛 `PluginDependencyMissingError`。`requires_operations` / `requires_sources` 找不到提供方也同理。
- **Operation / Source 依赖会反解为插件依赖**——`A.requires_operations=("todo:default.create",)` 且该 op 由 B 提供，则 DAG 中自动加入 `A -> B`。
- **Operation / Source 静态声明冲突会在装载前失败**——两个 plugin 同时 `provides_operations` 同一 `op_id` 或 `provides_sources` 同一 `source_id` 会抛 `OperationProvisionConflictError` / `SourceProvisionConflictError`。
- **环检测来自 `graphlib.CycleError`**。`exc.args[1]` 是参与环的节点列表。

### 配置校验：msgspec.convert

`load_into(..., configs=Mapping[str, object] | None)` 接受两种配置输入：

- 已经是目标 `cls.Config` 实例
- 原始 mapping / payload，由 `msgspec.convert(raw, type=cls.Config)` 转成 struct

转换失败时，loader 抛 `PluginLoadFailedError`，其中 `error.code == Errs.PLUGIN_CONFIG_INVALID`，`error.evidence` 会记录 plugin id、原始配置类型和 msgspec 异常摘要。这样 YAML / TOML 读取层未来只需要传入普通 mapping，schema 错误仍在装载阶段 fail-loud。

### 装载：配置转换 + 实例化 + on_load + attach

[loader.py](../../mutsukibot/core/loader.py)：

```python
async def load_into(
    self,
    agent: Agent,
    plugin_classes: Iterable[type[Plugin]],
    configs: Mapping[str, object] | None = None,
) -> None:
    configs = configs or {}
    by_id: dict[str, type[Plugin]] = {cls.id: cls for cls in plugin_classes}

    deps_map = {
        pid: tuple(d.plugin_id for d in cls.requires_plugins)
        for pid, cls in by_id.items()
    }
    order = _toposort(deps_map)

    for pid in order:
        cls = by_id[pid]
        raw_cfg = configs[pid] if pid in configs else cls.Config()
        cfg = _resolve_config(pid, cls, raw_cfg)
        scope = PluginScope(owner=pid)
        instance = cls(
            agent=agent,
            config=cfg,
            scope=scope,
            services=agent.services,
            bus=agent.bus,
        )
        await instance.on_load()
        agent.attach_plugin(instance, scope)
```

每个插件分得一个独立 `PluginScope(owner=pid)`。`on_load()` 成功后才 `agent.attach_plugin(...)`，避免半加载插件污染 `agent.plugins`；`attach_plugin` 会把 `@command` 生成的 `CommandSpec` 注册为 dispatcher Operation。

### 卸载：反序

[loader.py:122-133](../../mutsukibot/core/loader.py#L122-L133)：

```python
async def unload_from(self, agent: Agent) -> None:
    while agent.plugins:
        entry = agent.plugins.pop()
        agent.detach_plugin(entry.plugin)
        try:
            await entry.plugin.on_unload()
        finally:
            await entry.scope.close()
```

按加载反序卸载（`agent.plugins.pop()`）。`on_unload` 出错也保证 `scope.close()` 跑（finally）。`scope.close()` 抛 `HandleLeakError` 会传播，让上层看到泄漏。

`PluginRegistry` 只保存类注册，不保存当前实例；卸载实例不会触碰 `PluginRegistry`。Operation / Source 的运行时注册由 `PluginScope.close()` 触发 dispatcher 反注册回调清理。

### 结构化错误

常见 loader 异常：

```python
class PluginCycleError(Exception):
    def __init__(self, cycle: list[str], err: Error) -> None:
        ...
        self.cycle = cycle
        self.error = err

class PluginDependencyMissingError(Exception):
    missing: list[tuple[str, str]]
    error: Error

class PluginLoadFailedError(Exception):
    plugin_id: str
    error: Error

class PluginNotFoundError(KeyError):
    pass
```

环、依赖缺失、配置错误、`on_load` 失败都带结构化 `Error`，直接 `e.error` 拿到对应 `Errs.*`。

## 用法示例

声明依赖：

```python
from mutsukibot.contracts.plugin import PluginDep

class WebSearchPlugin(Plugin[Cfg]):
    id = "web-search"
    version = "0.1.0"
    capabilities = [Capability(name=Caps.NETWORK_EGRESS)]
    requires_plugins = [PluginDep(plugin_id="http-client")]
    Config = Cfg
```

装载：

```python
from mutsukibot.core.loader import PluginLoader

loader = PluginLoader()  # 默认从 entry_points 发现
classes = loader.discover()
await loader.load_into(agent, classes)
```

或者显式传：

```python
loader = PluginLoader(allow={"web-search", "http-client"})
await loader.load_into(agent, [WebSearchPlugin, HttpClientPlugin])
# loader 会先 load HttpClientPlugin 再 load WebSearchPlugin
```

传原始配置：

```python
await loader.load_into(
    agent,
    [WebSearchPlugin],
    configs={"web-search": {"timeout": 3.0}},
)
```

捕获环：

```python
from mutsukibot.core.loader import PluginCycleError

try:
    await loader.load_into(agent, [A, B])  # A 依赖 B，B 依赖 A
except PluginCycleError as e:
    print(e.cycle)             # ["A", "B"]
    print(e.error.evidence)    # {"remaining": "A,B"}
```

捕获配置错误：

```python
from mutsukibot.contracts.error import Errs
from mutsukibot.core.loader import PluginLoadFailedError

try:
    await loader.load_into(agent, [WebSearchPlugin], configs={"web-search": {"timeout": "slow"}})
except PluginLoadFailedError as e:
    assert e.error.code == Errs.PLUGIN_CONFIG_INVALID
```

## 常见陷阱

- **依赖必须在本次装载列表内**。显式装载子集时，记得把 `requires_plugins` 和 `requires_operations` / `requires_sources` 的提供方一并传入。
- **`configs={pid: cfg}` 必须 pid 完全匹配**。错拼成 `web_search` vs `web-search` 会被忽略，loader 退回去用 `cls.Config()` 默认值。当前不会把"未知 config key"当错误。
- **`load_into` 不是幂等**。重复调用会重复实例化并重新注册 Operation / Source；冲突会在 dispatcher 或 loader 静态闸口暴露。
- **`unload_from` 是反序的全卸载**。没有"只卸载某一个插件"的接口；要部分卸载需自己实现"找到那条 entry → detach → close"的最小 sequence，并维护好剩余依赖图。
- **`PluginRegistry` 是进程级类注册表**。多 Agent 可以共享同一插件类；实例级 Operation / Source 状态在每个 Agent 的 `Dispatcher` 内部维护。
- **environment 里的 entry_points 取决于 site-packages 状态**。开发时用 `uv pip install -e .` 让 entry_points 立刻生效；改 `pyproject.toml` 后记得重新 install。
