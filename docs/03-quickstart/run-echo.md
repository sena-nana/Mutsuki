# 跑通 Echo

## 目标

用 MutsukiBot 自带的 `EchoPlugin` + `InMemoryAdapter` 在十秒内走完一遍消息闭环：构造 Agent → 装载插件 → 启动调度器 → 投消息 → 拿回响应 → 看 trace。

## 准备

完成 [安装](../02-installation/installation.md) —— 你应当有一个能跑 `pytest tests/` 的开发环境。

## 一行命令

仓库自带冒烟入口（[mutsukibot/plugins/echo/smoke.py](../../mutsukibot/plugins/echo/smoke.py)）：

```bash
uv run python -m mutsukibot.plugins.echo.smoke
```

预期输出（来自 [v0.1 报告](../../plans/version-reports/v0.1.md)）：

```
[smoke] agent smoke-agent phase=spawn
[smoke] loaded plugins: ['mutsukibot-echo']
[smoke] phase=awake
[smoke] outbox -> 'echo: hello\n'
[smoke] phase=stop; trace at .../mutsukibot-echo-smoke.jsonl
```

最后一行的路径里能找到一个 JSONL trace —— 每条 span 一行。`cat` 出来类似：

```json
{"trace_id":"trace_xxx","span_id":"span_yyy","parent_span_id":null,"name":"plugin.mutsukibot-echo.echo","start":1700000000.0,"end":1700000000.0,"status":"ok","attributes":{"agent_id":"smoke-agent"}}
```

## 拆开看

`smoke.py` 的关键步骤：

```python
agent = Agent(
    agent_id=AgentId("smoke-agent"),
    clock=SystemClock(),
    id_gen=NanoIdGen(),
    rng=SeededRng(seed=0),
)
trace_path = Path(gettempdir()) / "mutsukibot-echo-smoke.jsonl"
writer = JsonlTraceWriter(trace_path)
writer.attach(agent.bus)

loader = PluginLoader(allow={EchoPlugin.id})
await loader.load_into(agent, [EchoPlugin])

scheduler = AgentScheduler(agent)
await scheduler.start()

adapter = InMemoryAdapter()
await adapter.send_text(agent, "echo hello")
await asyncio.sleep(0.3)
msgs = await adapter.drain_outbox(agent, timeout=0.5)

await scheduler.stop()
await loader.unload_from(agent)
writer.detach()
```

每一行做的事：

| 行 | 子系统 | 详见 |
|---|---|---|
| `Agent(...)` | 构造一等 Agent | [Agent 与生命周期](../04-guide/agent-and-lifecycle.md) |
| `JsonlTraceWriter` | 旁路订阅 trace | [Trace 与 Span](../04-guide/trace-and-span.md) |
| `PluginLoader` | 按 DAG 装载插件 | [插件 DAG 加载](../05-advanced/plugin-loader-dag.md) |
| `AgentScheduler.start` | 启动 tick 循环，进入 AWAKE | [Agent 与生命周期](../04-guide/agent-and-lifecycle.md) |
| `adapter.send_text` | 入站 Message → `agent.inbox` | [写一个 Adapter](../06-developer/writing-adapter.md) |
| `drain_outbox` | 从 `agent.outbox` 收响应 | 同上 |
| `scheduler.stop` + `loader.unload_from` | 反序卸载，触发 scope 关闭 | [热重载与泄漏检测](../05-advanced/hot-reload-and-leak.md) |

## 内部都发生了什么

把上面的"echo hello"沿调用链画出来：

```
adapter.send_text("echo hello")
  └─ Message(parts=[ContentPart(TEXT, "echo hello")])
  └─ agent.inbox.put(msg)

(scheduler tick 取到 msg)
AgentScheduler._handle_message(msg)
  ├─ shlex.split("echo hello") → ["echo", "hello"]
  ├─ agent.find_command("echo") → CommandTarget(plugin=EchoPlugin, ...)
  ├─ 构造 TraceContext + AgentContext
  ├─ check_capabilities(declared={READ_MESSAGE, SEND_MESSAGE}, required=()) → 通过
  ├─ Perms.PUBLIC.check(ctx) → True
  ├─ extras = {"text": "hello"}（按 schema properties 顺序对齐）
  ├─ marker.dependent.solve(ctx, bound_self=plugin, text="hello")
  │   └─ EchoPlugin.echo(self, text="hello", count=1)
  │      └─ return "echo: hello\n"
  ├─ outbox.put(Message(parts=[ContentPart(TEXT, "echo: hello\n")]))
  └─ bus.publish("trace.span", TraceSpan(name="plugin.mutsukibot-echo.echo", ...))

(JsonlTraceWriter 收到 trace.span，写一行 JSON 到文件)
```

## 自己改一行试试

打开 [mutsukibot/plugins/echo/__init__.py](../../mutsukibot/plugins/echo/__init__.py)：

```python
class _EchoConfig(msgspec.Struct, kw_only=True):
    prefix: str = "echo: "
```

改成 `prefix: str = "你好："`，重新跑 smoke：

```
[smoke] outbox -> '你好：hello\n'
```

或者投个带参数的命令——echo 接受 `count`（1-10）：

```python
# 把 smoke.py 里
await adapter.send_text(agent, "echo hello")
# 改成
await adapter.send_text(agent, "echo hi 3")
```

输出会变成：

```
[smoke] outbox -> 'echo: hi\necho: hi\necho: hi\n'
```

`3` 被 scheduler 按 `parameters_schema` 类型强转成 `int`（[scheduler.py:211-219](../../mutsukibot/runtime/scheduler.py#L211-L219)）。

## 观察 trace

JSONL trace 是诊断 MutsukiBot 业务的主要入口。打开 `/tmp/mutsukibot-echo-smoke.jsonl`（或 Windows 的 `%TEMP%/mutsukibot-echo-smoke.jsonl`），每行是一个 span：

```json
{
  "trace_id": "trace_xxx",
  "span_id": "span_yyy",
  "parent_span_id": null,
  "name": "plugin.mutsukibot-echo.echo",
  "start": 1700000000.123,
  "end": 1700000000.124,
  "status": "ok",
  "attributes": {"agent_id": "smoke-agent"}
}
```

`status` 在命令成功时是 `ok`，抛异常时是 `error`。试试把命令名改错（比如 `echoo`），观察 outbox 与 trace：

```
[smoke] outbox -> '[error capability.not_declared] {...}'
```

trace 里看不到 span —— 因为命令未找到时 scheduler 直接 `_emit_error` 返回，不进入 try/finally 块（[scheduler.py:88-99](../../mutsukibot/runtime/scheduler.py#L88-L99)）。这是 v0.1 的已知行为。

## 下一步

跑通了，接下来 → [写第一个插件](first-plugin.md)。
