# 跑通 Echo（Python reference）

本 quickstart 使用 `python/reference-mutsukicore` 的旧 Python reference 插件链路。
如果目标是验证当前 Rust core smoke，请以根目录 `cargo test` 和
`mutsukicore-runtime-host` 的 native smoke 覆盖为准，而不是 Python `AgentScheduler`。

## 目标

用 `EchoPlugin` + `InMemoryEndpointPlugin` 走完一遍 Python reference 消息闭环：构造 Agent → 装载 endpoint 与命令插件 → 启动 Python reference `AgentScheduler` → publish 一条 IM `Message` → 从 outbox 拿回响应 → 看 trace。

## 一行命令

仓库自带冒烟入口：

```bash
uv run python -m mutsukicore.plugins.echo.smoke
```

预期输出会列出已加载插件、dispatcher operations / sources，以及一条 `echo: hello` 回执。`source.source_id` 应保持为 `inmemory:default`，这证明 v0.2 的出站消息没有丢失 transport 来源。

## 拆开看

`smoke.py` 的关键步骤：

```python
agent = Agent(
    agent_id=AgentId("smoke-agent"),
    clock=SystemClock(),
    id_gen=NanoIdGen(),
    rng=SeededRng(seed=0),
    accepts=(Scopes.IM_TEXT.to_rule(),),
)

loader = PluginLoader(allow={InMemoryEndpointPlugin.id, EchoPlugin.id})
await loader.load_into(agent, [InMemoryEndpointPlugin, EchoPlugin])

scheduler = AgentScheduler(agent)
await scheduler.start()

inmem = next(
    p.plugin for p in agent.plugins
    if isinstance(p.plugin, InMemoryEndpointPlugin)
)
await inmem.send_text("echo hello")
msgs = await inmem.drain_outbox(timeout=0.5)
```

每一行做的事：

| 行 | 子系统 | 详见 |
|---|---|---|
| `accepts=(Scopes.IM_TEXT.to_rule(),)` | Agent 显式声明接收 IM 文本 envelope | [Agent 与生命周期](../04-guide/agent-and-lifecycle.md) |
| `InMemoryEndpointPlugin` | 注册 `inmemory:default` Source | [写一个 transport plugin](../06-developer/writing-transport-plugin.md) |
| `EchoPlugin` | `@command` 自动注册 `mutsukicore-echo.echo` Operation | [插件定义](../04-guide/plugin-definition.md) |
| `send_text` | 构造 `Message` 并 `dispatch.publish` | [Dispatcher API](../07-api/dispatcher.md) |
| `drain_outbox` | 从 Agent outbox 读取命令响应 | [测试夹具](../06-developer/testing-fixtures.md) |

## 内部链路

```text
InMemoryEndpointPlugin.send_text("echo hello")
  -> Message(source.source_id="inmemory:default")
  -> Dispatcher.publish(message)
  -> AgentRegistry.iter_accepting(message)
  -> agent.inbox.put(message)

Python reference AgentScheduler._loop
  -> plugin.on_envelope hooks
  -> command text route: "echo" -> "mutsukicore-echo.echo"
  -> Dispatcher.invoke(...)
  -> EchoPlugin.echo(...)
  -> agent.outbox.put(Message(source.source_id="inmemory:default"))
```

v0.2 起命令也是 Operation；`@command` 只是声明糖，最终由 PluginMeta 自动汇入 `provides_operations`。

## 下一步

跑通后看 [第一个插件](first-plugin.md)，或直接看 [写一个 transport plugin](../06-developer/writing-transport-plugin.md) 了解真实平台接入方式。
