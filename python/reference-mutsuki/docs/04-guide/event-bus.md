# 事件总线

## 这是什么

`Bus` 是 Agent 持有的进程内 pub/sub。订阅按 `event_type`（字符串）分桶，每桶再分 `direct` / `deferred` 两条独立列表，避免每次 publish 重新过滤。

代码：[mutsuki/core/bus.py](../../mutsuki/core/bus.py)。

## 解决什么问题

观察者层（trace 写入器、审计、性能采样）必须能旁路订阅事件而**不被任何上游层依赖**。如果改成同步回调，每加一个观察者都要改 publisher；如果改成持久消息队列，又拖累延迟。Bus 是中间形态：

- 进程内、零序列化
- 默认并发派发（不阻塞 publisher）
- 提供"快路径直调"模式给延迟敏感链路（Yume 的 `thought → kernel → runtime` 这种 sub-ms 链）

## 怎么工作

### 数据结构

[bus.py:33-41](../../mutsuki/core/bus.py#L33-L41)：

```python
@dataclass(slots=True)
class _EventBucket:
    direct: list[_Subscription] = field(default_factory=list)
    deferred: list[_Subscription] = field(default_factory=list)


@dataclass(slots=True)
class Bus:
    _subs: dict[str, _EventBucket] = field(default_factory=dict)
```

每个事件类型一个 bucket，bucket 内 direct / deferred 永久分开。subscribe 时根据 `direct` flag 落到对应 list。

### subscribe

[bus.py:43-61](../../mutsuki/core/bus.py#L43-L61)：

```python
def subscribe(
    self, event_type: str, handler: EventHandler, *, direct: bool = False
) -> Callable[[], None]:
    sub = _Subscription(sub_id=next(_sub_id_counter), handler=handler)
    bucket = self._subs.setdefault(event_type, _EventBucket())
    target = bucket.direct if direct else bucket.deferred
    target.append(sub)

    def _unsubscribe() -> None:
        ...
    return _unsubscribe
```

返回的 `_unsubscribe` 是闭包，捕获 `sub.sub_id`。**插件必须把它登记到 `scope.add_subscription(unsub)`**，否则插件卸载后订阅还在，scope 检查到 cleanup 函数从未执行 —— 不过 v0.1 还没有"未注册副作用 = 违规"的运行时守卫，靠 code review 拦。

### publish：两种派发

[bus.py:63-70](../../mutsuki/core/bus.py#L63-L70)：

```python
async def publish(self, event_type: str, payload: object) -> None:
    bucket = self._subs.get(event_type)
    if bucket is None:
        return
    for sub in bucket.direct:
        await sub.handler(payload)
    if bucket.deferred:
        await asyncio.gather(*(s.handler(payload) for s in bucket.deferred))
```

| 模式 | 调度方式 | 适用场景 |
|---|---|---|
| `direct=False`（默认） | `asyncio.gather` 并发 | 大多数订阅；不阻塞 publisher |
| `direct=True` | publisher 内联 await，按注册顺序串行 | sub-ms 延迟链路（thought → kernel → runtime） |

direct 模式的代价：handler 抛异常会直接传到 publisher。deferred 模式由 `asyncio.gather` 把异常聚合后抛 —— 同一组 handler 的失败不互相影响。

### unsubscribe 的实现

[bus.py:51-60](../../mutsuki/core/bus.py#L51-L60)：闭包遍历该事件类型的 direct + deferred 两条 list，按 sub_id 找到第一个匹配项 `del`。这是 O(n) on n 订阅数；当前规模下可接受，热点订阅不超过几十个。

## 用法示例

订阅 + 自动清理：

```python
async def on_load(self) -> None:
    async def on_trace(payload: object) -> None:
        # payload 是发布者送进来的，按 event_type 约定类型；这里假设是 TraceSpan
        ...
    unsub = self.bus.subscribe("trace.span", on_trace)
    self.scope.add_subscription(unsub)
```

发布事件：

```python
@command()
async def publish_demo(self, ctx: AgentContext, msg: str) -> str:
    await ctx.bus.publish("my.event", {"text": msg, "ts": ctx.clock.now()})
    return "ok"
```

direct 订阅（典型用法：旁路写 trace 时不增加延迟，但你必须保证 handler 极快）：

```python
unsub = bus.subscribe("trace.span", _fast_handler, direct=True)
```

观察者样例：[`JsonlTraceWriter`](../../mutsuki/observability/trace.py#L30-L47) 订阅 `trace.span`，把每条 span 落 JSONL；写失败时不阻断 publisher，而是把异常发回 bus 上的 `trace.write_failed` 事件 —— 符合 hard rule #8 "不允许默默吞错"。

## 常见陷阱

- **不要从同步代码 `bus.publish(...)` 然后丢弃 awaitable**。`publish` 是 async；不 await 等于没发。
- **`event_type` 是字符串，没有注册表**。约定按 `domain.subdomain.action` 命名（如 `trace.span`、`yume.thought.emitted`）。框架内置事件名集中在 [scheduler.py](../../mutsuki/runtime/scheduler.py) 与 [observability/](../../mutsuki/observability/) 里。
- **`direct=True` handler 慢会拖累整条调用链**。Yume kernel 一类延迟敏感路径才用；trace / audit 一律 deferred。
- **deferred handler 的失败被 `asyncio.gather` 默认行为聚合后抛出**——publisher 看到的是一个 ExceptionGroup（Python 3.11+ 行为），不是单个异常。要单独失败"包住"，handler 自己 try/except。
- **subscribe 与 unsubscribe 不是线程安全的**——`_subs` / list 都是普通 Python 容器。Agent 默认单事件循环单线程，没问题；如果你跨线程用，自己加锁。
- **subscribe 顺序决定 direct 模式的执行顺序**。两个 direct handler 都改 payload 的话，先注册的先看到原始值。
