# 写一个 Transport Plugin

## 这是什么

v0.2 起 MutsukiBot 不再提供独立 Adapter 抽象。外部协议接入统一写成 plugin：在 `on_load` 中注册 Source / Operation，并把 socket、SDK client、server 等 I/O 资源通过 `Handle[T]` attach 到 `PluginScope`。

内置参考：

- [InMemoryEndpointPlugin](../../mutsukibot/plugins/inmemory_endpoint/__init__.py) —— 测试用 IM endpoint。
- [OneBotV11Plugin](../../mutsukibot/plugins/onebot_v11/__init__.py) —— OneBot v11 反向 WebSocket reference plugin。

## 最小结构

```python
class MyTransportPlugin(Plugin[MyConfig]):
    id: ClassVar[str] = "my-transport"
    version: ClassVar[str] = "0.1.0"
    capabilities: ClassVar[list[Capability]] = [
        Capability(name=Caps.READ_MESSAGE),
        Capability(name=Caps.SEND_MESSAGE),
        Capability(name=Caps.NETWORK_EGRESS),
    ]
    provides_sources: ClassVar[tuple[SourceDescriptor, ...]] = (MY_SOURCE,)
    provides_operations: ClassVar[tuple[OperationDescriptor, ...]] = (MY_SEND_OP,)
    Config = MyConfig

    async def on_load(self) -> None:
        self.agent.dispatch.register_source(
            MY_SOURCE,
            plugin_scope=self.scope,
            plugin_id=self.id,
        )
        self.agent.dispatch.register_operation(
            MY_SEND_OP,
            handler=self._send,
            perms=Perms.PUBLIC.to_rule(),
            plugin_scope=self.scope,
        )
```

## 入站事件

transport plugin 收到外部事件后，把协议字段翻译成内部 `Message` / `Envelope`，再调用：

```python
await self.agent.dispatch.publish(message)
```

`message.source.source_id` 必须引用已注册的 `SourceDescriptor.source_id`。Agent 是否接收由 `Agent.accepts` 的 `ScopeRule` 决定。

## 出站调用

发送消息或调用外部 API 暴露为 Operation：

```python
await ctx.dispatch.invoke("onebot:v11.default.send_msg", payload)
```

Operation handler 直接 await 外部 I/O。失败时抛 `OperationInvokeError(Error(...))`，不要吞异常返回默认值。

## I/O 资源

禁止 plugin 字段裸持 raw socket / SDK client / websocket server。正确做法：

```python
self._server_handle = RefCountedHandle(
    target=server,
    descriptor=RefDescriptor(...),
    finalizer=lambda target: target.close(),
)
self._server_handle.attach_to(self.scope)
self.scope.add_dispose(close_server)
```

hard rule #14 lint 会扫描 `Plugin` 子类字段，发现 `socket.socket` / `aiohttp.ClientSession` / websocket connection/server 等 raw I/O 类型时失败。字段应持 `Handle[Any] | None`。

## 常见陷阱

- Source / Operation 必须先在类级 `provides_sources` / `provides_operations` 静态声明，再在 `on_load` 注册。
- `source_id` / `op_id` 在 v0.2 是静态 manifest 的一部分；不要在配置里生成任意动态 id，除非同时扩展 manifest 机制。
- 外部协议类型只能放在 reference plugin 内，不得写进 `mutsukibot.core` 或 `mutsukibot.contracts`。
- plugin 卸载后 dispatcher 中不得残留 Operation / Source；测试使用 `assert_dispatcher_clean_after_unload`。
