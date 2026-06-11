# 服务容器

## 这是什么

`ServiceContainer` 是 Agent 持有的服务注册表 —— 按 `(契约类型, 可选名字)` 索引。插件通过命令签名里的 `Inject()` 默认值，让框架自动从容器解析出实例。

代码：[mutsukicore/core/container.py](../../mutsukicore/core/container.py)。

## 解决什么问题

插件之间禁止直接 `import` 实现模块（hard rule #3）—— 否则一个 LLM provider 插件的实现细节会绑死所有调用方。Service 是它们之间的耦合点：**契约类型作为身份**，谁来实现、是不是同一个进程、是不是同一个 Agent，调用方都不需要知道。

## 怎么工作

### 数据结构

[container.py:17-21](../../mutsukicore/core/container.py#L17-L21)：

```python
class ServiceContainer:
    def __init__(self) -> None:
        self._by_type: dict[type, list[tuple[str | None, Any]]] = {}
```

按契约类型分桶；每桶是 `[(name, instance), ...]`。允许同一契约下挂多个实现（按 name 区分）。

### 注册

[container.py:23-30](../../mutsukicore/core/container.py#L23-L30)：

```python
def register(
    self,
    contract: type,
    instance: Any,
    *,
    name: str | None = None,
) -> None:
    self._by_type.setdefault(contract, []).append((name, instance))
```

通常在插件的 `on_load` 里注册，并把 `unregister` 包装成 cleanup 登记到 scope（详见 [PluginScope](plugin-scope.md)）。

### 解析

[container.py:38-51](../../mutsukicore/core/container.py#L38-L51)：

```python
def resolve(self, contract: type, *, name: str | None = None) -> Any:
    bucket = self._by_type.get(contract)
    if not bucket:
        raise ServiceNotFoundError(...)
    if name is not None:
        for n, inst in bucket:
            if n == name:
                return inst
        raise ServiceNotFoundError(...)
    return bucket[0][1]
```

不指定 name 就拿第一个。`ServiceNotFoundError` 是 `KeyError` 的子类。文本命令 reference extension 的异常分类器会把它映射成 `Errs.SERVICE_NOT_FOUND` 并标 `reason="service_not_found"`。

### Inject() 注入流程

`Inject()` 是 [contracts/plugin.py:37-44](../../mutsukicore/contracts/plugin.py#L37-L44) 定义的 sentinel：

```python
@dataclass(frozen=True, slots=True)
class Inject:
    name: str | None = None
```

在命令签名里以 **默认值** 形式出现：`svc: SomeService = Inject()` 或 `svc: SomeService = Inject(name="primary")`。

Dependent 的 `ServiceParam` 看到默认值是 `Inject` 实例就认领（[dependency.py:138-142](../../mutsukicore/core/dependency.py#L138-L142)），调用时执行：

```python
ann = _strip_annotated(self.info.annotation)
return ctx.services.resolve(ann, name=self.inject.name)
```

也就是说**注解类型 = 解析键**，default 里的 `Inject(name=...)` 提供可选的命名。

### by-value vs by-ref

`ServiceMode`（[contracts/service.py:9-11](../../mutsukicore/contracts/service.py#L9-L11)）有两个值：

- `BY_VALUE` —— 服务实例本身可序列化，跨进程也能传
- `BY_REF` —— 服务持有非可序列化资源（GPU handle、KV cache 槽……），不能跨进程

当前 `ServiceContainer` 不区分这两种 —— 它只解析进程内对象，模式只是 manifest 层（`ServiceDep.mode`）的元数据。容器跨进程序列化拦截是后续版本的工作（详见 [附录 · 未实现](../appendix/roadmap-and-not-yet.md)）。

## 用法示例

注册侧（一个提供 HTTP 客户端的插件）：

```python
class HttpClientPlugin(Plugin[_HttpConfig]):
    id = "http-client"
    version = "0.1.0"
    capabilities = [Capability(name=Caps.NETWORK_EGRESS)]
    Config = _HttpConfig

    async def on_load(self) -> None:
        self._client = httpx.AsyncClient(timeout=self.config.timeout)
        self.services.register(HttpClient, self._client)
        self.scope.add_service_registration(
            lambda: self.services.unregister(HttpClient, self._client)
        )

    async def on_unload(self) -> None:
        await self._client.aclose()
```

消费侧：

```python
class WebSearchPlugin(Plugin[_Cfg]):
    id = "web-search"
    version = "0.1.0"
    capabilities = [Capability(name=Caps.NETWORK_EGRESS)]
    Config = _Cfg

    @command()
    async def search(
        self,
        query: str,
        client: HttpClient = Inject(),
    ) -> str:
        """模糊检索。"""
        resp = await client.get(f"https://api/?q={query}")
        return resp.text
```

声明侧依赖（让 loader 能保证装载顺序）：

```python
from mutsukicore.contracts.plugin import PluginDep, ServiceDep

class WebSearchPlugin(Plugin[_Cfg]):
    requires_plugins = [PluginDep(plugin_id="http-client")]
    requires_services = [ServiceDep(name="", contract_id="HttpClient")]
    ...
```

## 常见陷阱

- **`unregister` 不会自动发生**。即便插件被卸载，scope 也不会自己卸载服务 —— 你必须把 unregister 显式登记到 `scope.add_service_registration(...)`。否则服务会留在容器里悬空。
- **同契约多实例时，`resolve(name=None)` 拿第一个**。这通常是你想要的（"主实例"），但要避免依赖隐含的注册顺序。多实例场景请显式给名字。
- **解析失败抛的是 `ServiceNotFoundError`（`KeyError` 子类）**。在文本命令里这会被 command router 包成结构化 `Error`；在 `on_load` 里这会直接传播让 loader 装载失败。
- **服务的契约类型推荐用 `Protocol` 或 ABC**，不要用具体类。这样替换实现时签名不变。
- **不要把 `AgentContext` 注册成服务**。它是 per-call 对象，不是单例；要拿全局共享 state 用真服务。
