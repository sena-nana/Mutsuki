# 插件 DAG 加载

## 这是什么

`PluginLoader` 负责发现、按依赖拓扑排序、装载、卸载插件。装载顺序由 `requires_plugins` 形成的 DAG 决定，存在环则拒绝启动。

代码：[nanobot/core/loader.py](../../nanobot/core/loader.py)。

## 解决什么问题

插件之间通过 service / bus 通信，但服务**实例化**仍有顺序依赖：用户搜索插件依赖 HTTP 客户端插件，HTTP 必须先 `on_load` 注册服务，搜索插件 `on_load` 时才解析得到。靠人工排顺序不可持续 —— 一旦插件多了，加载 / 卸载顺序就成了隐含约定。DAG 拓扑排序把它形式化。

## 怎么工作

### 发现：entry_points

[loader.py:77-90](../../nanobot/core/loader.py#L77-L90)：

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

约定：`pyproject.toml` 里声明 `[project.entry-points."nanobot.plugins"]`：

```toml
[project.entry-points."nanobot.plugins"]
echo = "nanobot.plugins.echo:EchoPlugin"
```

允许传 `allow={...}` 白名单只装一部分，便于测试。

### 拓扑排序：graphlib

[loader.py:43-64](../../nanobot/core/loader.py#L43-L64)：

```python
def _toposort(items: dict[str, tuple[str, ...]]) -> list[str]:
    graph = {node: tuple(d for d in deps if d in items) for node, deps in items.items()}
    sorter = graphlib.TopologicalSorter(graph)
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

- **外部依赖被过滤**——`requires_plugins=[PluginDep(plugin_id="some-other")]` 但 `some-other` 没在本次装载列表里，loader 不会失败，只是把它当不存在。这避免"装载子集"必须先装载所有依赖；副作用是拼写错误的依赖名不会立即报错。
- **环检测来自 `graphlib.CycleError`**。`exc.args[1]` 是参与环的节点列表。

### 装载：构造 Config + 实例化 + on_load

[loader.py:92-120](../../nanobot/core/loader.py#L92-L120)：

```python
async def load_into(
    self,
    agent: Agent,
    plugin_classes: Iterable[type[Plugin]],
    configs: dict[str, msgspec.Struct] | None = None,
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
```

每个插件分得一个独立 `PluginScope(owner=pid)`。`attach_plugin` 同时把命令登记到 agent 的 `_command_index`。

### 卸载：反序

[loader.py:122-133](../../nanobot/core/loader.py#L122-L133)：

```python
async def unload_from(self, agent: Agent) -> None:
    while agent.plugins:
        entry = agent.plugins.pop()
        agent.detach_plugin(entry.plugin)
        try:
            await entry.plugin.on_unload()
        finally:
            await entry.scope.close()
            PluginRegistry.unregister(entry.plugin.id)
            PluginRegistry.register(entry.plugin.id, type(entry.plugin))
```

按加载反序卸载（`agent.plugins.pop()`）。`on_unload` 出错也保证 `scope.close()` 跑（finally）。`scope.close()` 抛 `HandleLeakError` 会传播，让上层看到泄漏。

最后那句"unregister 实例 + register 类"是个细节：`PluginRegistry` 同时承载"已知插件类"与"当前装载的插件实例" —— 卸载实例后要把类登记回去，否则下次再装载会因为找不到注册项失败。

### PluginCycleError 与 PluginNotFoundError

[loader.py:32-40](../../nanobot/core/loader.py#L32-L40)：

```python
class PluginCycleError(Exception):
    def __init__(self, cycle: list[str], err: Error) -> None:
        ...
        self.cycle = cycle
        self.error = err

class PluginNotFoundError(KeyError):
    pass
```

环错误自带结构化 `Error`，直接 `e.error` 拿到 `Errs.PLUGIN_CYCLE`。

## 用法示例

声明依赖：

```python
from nanobot.contracts.plugin import PluginDep

class WebSearchPlugin(Plugin[Cfg]):
    id = "web-search"
    version = "0.1.0"
    capabilities = [Capability(name=Caps.NETWORK_EGRESS)]
    requires_plugins = [PluginDep(plugin_id="http-client")]
    Config = Cfg
```

装载：

```python
from nanobot.core.loader import PluginLoader

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

捕获环：

```python
from nanobot.core.loader import PluginCycleError

try:
    await loader.load_into(agent, [A, B])  # A 依赖 B，B 依赖 A
except PluginCycleError as e:
    print(e.cycle)             # ["A", "B"]
    print(e.error.evidence)    # {"remaining": "A,B"}
```

## 常见陷阱

- **拼写错误的依赖名不会立刻报错**——它会被 `_toposort` 过滤掉。在测试里覆盖装载顺序断言（"WebSearch 在 HttpClient 之后"）能拦到。
- **`configs={pid: cfg}` 必须 pid 完全匹配**。错拼成 `web_search` vs `web-search` 会被忽略，loader 退回去用 `cls.Config()` 默认值——配置悄无声息地不生效。
- **`load_into` 不是幂等**。重复调用会重复实例化，命令索引也会重复登记 —— 用 `setdefault` 保护了第一次的优先级，但仍然是浪费。
- **`unload_from` 是反序的全卸载**。没有"只卸载某一个插件"的接口；要部分卸载需自己实现"找到那条 entry → detach → close"的最小 sequence，并维护好剩余依赖图。
- **`PluginRegistry` 是进程级单例**——多个 Agent 装载同一个插件类时，卸载哪一个都会触发"unregister 实例 + register 类"，所以同一进程里多个 Agent 用同一插件**只工作于"全装全卸"**。当前实现限制；要多 Agent 真正隔离需要每 Agent 自己的 PluginRegistry。
- **environment 里的 entry_points 取决于 site-packages 状态**。开发时用 `uv pip install -e .` 让 entry_points 立刻生效；改 `pyproject.toml` 后记得重新 install。
