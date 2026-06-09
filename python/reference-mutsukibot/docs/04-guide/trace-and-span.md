# Trace 与 Span

## 这是什么

MutsukiBot 的因果链系统：dispatcher 调用、跨 Agent 调用、envelope consumer 和 ResourceHost 关键入口都会 emit `TraceSpan` 到事件总线；观察者订阅 `trace.span` 写到结构化 sink（JSONL、OTel、其他）。

代码：

- 上下文：[`TraceContext`](../../mutsukibot/core/context.py#L25-L29)
- 契约：[`TraceSpan`](../../mutsukibot/contracts/event.py#L19-L32)、[`Event`](../../mutsukibot/contracts/event.py#L35-L48)、[`SpanStatus`](../../mutsukibot/contracts/event.py#L14-L17)
- 默认观察者：[`JsonlTraceWriter`](../../mutsukibot/observability/trace.py) / [`JsonlTraceReader`](../../mutsukibot/observability/trace.py)
- 回放与契约断言：[`replay_trace_spans`](../../mutsukibot/testing/trace_replay.py) / [`contract_kit`](../../mutsukibot/testing/contract_kit.py)

## 解决什么问题

多插件链式调用时（`adapter → echo → bus → trace_writer`），出问题的方式有三种：

1. 看不到中间发生了什么 —— 没有结构化日志
2. 看到了但找不到关联 —— 每条日志独立，串不起来"谁触发了谁"
3. 串得起来但形态不稳定 —— 字符串 log 改一行 grep 全断

`trace_id / span_id / parent_span_id` 三段标识 + 标准 `TraceSpan` 结构解决全部三件事。

## 怎么工作

### TraceContext 三段

[context.py:25-29](../../mutsukibot/core/context.py#L25-L29)：

```python
@dataclass(slots=True)
class TraceContext:
    trace_id: TraceId
    span_id: SpanId
    parent_span_id: SpanId | None = None
```

| 字段 | 含义 | 谁分配 |
|---|---|---|
| `trace_id` | 整个外部触发链路的标识 | 链路最外层（adapter / Python reference scheduler）一次性分配，传递不变 |
| `span_id` | 当前这一跳的标识 | 每一跳新建 |
| `parent_span_id` | 调用方的 span_id | 调用方传入；外部触发的根 span 为 None |

### Python reference Scheduler / Dispatcher emit span 的时机

[scheduler.py](../../mutsukibot/runtime/scheduler.py) 是 Python reference 层的兼容调度器，负责把 IM 文本解析成 Operation payload，并调用 dispatcher：

```python
ctx = self.agent.make_context(message=msg)
try:
    result = await self.agent.dispatch.invoke(op_id, payload, ctx=ctx)
    await self._emit_result(msg, str(result))
except OperationInvokeError as exc:
    await self._emit_error(msg, exc.error)
```

要点：

- **Operation 执行事实只由 dispatcher span 表达**。人类命令、跨 plugin RPC 和外部工具调用共享 `dispatch.invoke` span，避免 Python reference scheduler 再造一份 `plugin.<id>.<command>` 重复事实。
- **普通消息未匹配命令时**，Python reference scheduler 会发 `agent.scheduler.unmatched` span，且不写 outbox。
- **插件 envelope consumer** 由 Python reference scheduler 分发，但 span 通过 `core.trace.trace_span(...)` 统一创建，名称为 `plugin.<id>.on_envelope`。
- **start / end 来自 `agent.clock.now()`**——意味着 ManualClock 测试里 span 的时间也是确定的。

### Dispatcher / ResourceHost span

v0.3.1 起，以下入口会自动发 trace span，并临时切换当前 span 来保持父子链：

- `dispatch.invoke(...)`：`TraceSpan(name="dispatch.invoke")`
- `dispatch.invoke_in_agent(...)`：`TraceSpan(name="dispatch.invoke_in_agent")`
- `ResourceHost.acquire_for(...)`：`TraceSpan(name="resource_host.acquire")`
- `ResourceHost.release_for(...)`：`TraceSpan(name="resource_host.release")`
- `ResourceHost.get_handle_for(...)`：`TraceSpan(name="resource_host.get_handle")`

跨 Agent 调用时，目标 Agent 的 `dispatch.invoke` 沿用调用方的 `trace_id`，并把调用方 `dispatch.invoke_in_agent` span 作为 parent。

### TraceSpan 形态

[contracts/event.py:19-32](../../mutsukibot/contracts/event.py#L19-L32)：

```python
class TraceSpan(Contract):
    trace_id: TraceId
    span_id: SpanId
    parent_span_id: SpanId | None = None
    name: str = ""
    start: float = 0.0
    end: float | None = None
    attributes: dict[str, str | int | float | bool] = {}
    status: SpanStatus = SpanStatus.OK
```

`attributes` 同样只接受标量（与 `Error.evidence` 一致）。

### Event 与 TraceSpan 的关系

[contracts/event.py:35-48](../../mutsukibot/contracts/event.py#L35-L48) 的 `Event` 是更通用的"内部事件"包装：

```python
class Event(Contract):
    id: str
    timestamp: float
    type: str
    source_plugin: str
    payload: msgspec.Raw
    trace_id: TraceId
    span_id: SpanId
    parent_span_id: SpanId | None = None
```

—— 它把 trace 三段揉进了通用事件里。旧 v0.1 Python reference scheduler 只 publish `TraceSpan`（不是 `Event`），但 `Event` 的形状已经预留：插件之间发布业务事件时可用它，让 trace 写入器一并处理。

### JsonlTraceWriter / Reader

[observability/trace.py](../../mutsukibot/observability/trace.py) 的标准订阅者：

- `attach(bus)`：打开文件、订阅 `trace.span`
- `detach()`：unsubscribe + 关文件
- 写失败时不阻塞 publisher，转发到 bus 上的 `trace.write_failed` 事件（[trace.py:36-45](../../mutsukibot/observability/trace.py#L36-L45)）

落盘格式（[trace.py](../../mutsukibot/observability/trace.py)）：每行一个 JSON object，含 trace_id / span_id / parent_span_id / name / start / end / status / attributes。`JsonlTraceReader.read_all()` 按同构格式读回 `TraceSpan`；记录缺字段或字段类型错误时抛 `TraceReplayError`，其中 `error.code == Errs.TRACE_RECORD_INVALID`。

### Trace replay / contract kit

`mutsukibot.testing.replay_trace_spans(...)` 不重放外部副作用，只验证已记录 span 的因果链，并返回稳定排序的 `TraceReplayFrame`：

- 拒绝同一 trace 内重复 `span_id`
- 拒绝 `end < start`
- 拒绝父链成环
- `require_known_parents=True` 时要求父 span 闭合在同一批记录中

v0.4 起，`mutsukibot.testing.contract_kit` 在 replay helper 之上提供更高层断言：`assert_trace_tree_closed(...)`、`assert_cross_agent_trace_chain(...)`、`assert_dispatcher_clean(...)`。

## 用法示例

读 trace（在测试里订阅 `trace.span`）：

```python
spans = []

async def collect(payload: object) -> None:
    spans.append(payload)

unsub = agent.bus.subscribe("trace.span", collect)
# ... 跑命令 ...
unsub()

assert spans[0].name == "dispatch.invoke"
assert spans[0].status == SpanStatus.OK
```

业务嵌套调用时手工构造子 span：

```python
@command()
async def outer(self, ctx: AgentContext) -> str:
    sub_span_id = SpanId(ctx.id_gen.next("span"))
    sub_ctx = AgentContext(
        agent_id=ctx.agent_id,
        agent_owner=ctx.agent_owner,
        clock=ctx.clock,
        id_gen=ctx.id_gen,
        rng=ctx.rng,
        services=ctx.services,
        scope=ctx.scope,
        bus=ctx.bus,
        trace_ctx=TraceContext(
            trace_id=ctx.trace_ctx.trace_id,         # 继承
            span_id=sub_span_id,                     # 新建
            parent_span_id=ctx.trace_ctx.span_id,    # 父为外层
        ),
        message=ctx.message,
    )
    span_start = ctx.clock.now()
    try:
        result = await self._inner(sub_ctx)
        return result
    finally:
        await ctx.bus.publish("trace.span", TraceSpan(
            trace_id=sub_ctx.trace_ctx.trace_id,
            span_id=sub_ctx.trace_ctx.span_id,
            parent_span_id=sub_ctx.trace_ctx.parent_span_id,
            name="outer.inner",
            start=span_start,
            end=ctx.clock.now(),
            attributes={"sub_call": True},
        ))
```

落盘观察：

```python
from pathlib import Path
from mutsukibot.observability import JsonlTraceWriter

writer = JsonlTraceWriter(Path("/tmp/trace.jsonl"))
writer.attach(agent.bus)
# ... 跑业务 ...
writer.detach()
```

读回并校验闭合父链：

```python
from mutsukibot.observability import JsonlTraceReader
from mutsukibot.testing.contract_kit import assert_trace_tree_closed

spans = JsonlTraceReader(Path("/tmp/trace.jsonl")).read_all()
frames = assert_trace_tree_closed(spans)
```

## 常见陷阱

- **不要复用 trace_id**。同一外部触发整链共享一个 trace_id；新触发要新分配（Python reference scheduler 自动做）。手工嵌套调用记得**继承** trace_id，不是新建。
- **start / end 用 `clock.now()`，不要用 `clock.monotonic()`**。span 字段是墙钟时间，给观察者做绝对时间排序与跨进程关联。
- **`attributes` 只接受标量**——结构化字段塞 `json.dumps(...)`。
- **`parent_span_id` 是 None 不一定是 bug**——根 span 就是 None。需要强制闭合父链时，用 `assert_trace_tree_closed(...)` 或 `replay_trace_spans(..., require_known_parents=True)`。
- **status 默认 OK**——如果手工 emit span 不显式设 status，它会被记为 OK；记得在 except 里设 `status=SpanStatus.ERROR`。
- **`JsonlTraceWriter` 是同步 IO**。在生产里如果 trace 量大，写盘可能成为 publisher 瓶颈（虽然 `subscribe` 默认 deferred 模式下 publisher 不阻塞，但仍会消耗事件循环）。需要异步写入要自己实现。
- **`trace.write_failed` 事件如果再失败就会无限循环**。`JsonlTraceWriter` 当前只对 `trace.span` 失败发 `trace.write_failed`；如果你订阅了 `trace.write_failed` 又抛错，注意不要循环订阅同一通道。
