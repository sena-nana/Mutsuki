# 测试夹具

## 这是什么

NanoBot 内置一组用于测试的夹具：`ManualClock` / `DeterministicIdGen` / `SeededRng` / `make_stub_handle` / `InMemoryAdapter`。它们让你能在没有真实平台、没有真实后端、没有真实 wall-clock 的情况下完整复现 Agent 闭环。

## 解决什么问题

如果测试只能跑端到端，每条测试都几秒；如果测试只能 mock 一切，那 mock 与真实实现的漂移会让 bug 在生产才被发现。NanoBot 的妥协：

- **协议层提供两种实现**——生产 / 测试用一套同样的 ABC，业务代码不感知差异
- **scope 与 lifecycle 完全实现**——测试里的 Agent 是真 Agent，跑真调度，真泄漏检测
- **adapter / handle 提供 stub 工厂**——避免引入真实平台 SDK 与 GPU

## 夹具一览

### ManualClock

[clock.py:39-96](../../nanobot/runtime/clock.py#L39-L96)。详见 [确定性运行时](../05-advanced/deterministic-runtime.md)。

```python
from nanobot.runtime import ManualClock

clock = ManualClock(start=1_700_000_000.0, max_pending_waiters=64)

# 业务里 await ctx.clock.sleep(60) —— 阻塞，等 advance
# 测试里：
clock.advance(30)   # 推进 30 秒；deadline 在范围内的 sleeper 被唤醒
clock.advance(30)   # 再 30 秒

n = clock.cancel_all()      # teardown 时唤醒残留
assert clock.pending_waiters == 0
```

### DeterministicIdGen

```python
from nanobot.runtime import DeterministicIdGen

gen = DeterministicIdGen(seed=0)
assert gen.next() == "00000000000000000000000001"
assert gen.next("trace") == "trace_00000000000000000000000002"
```

`seed` 决定起始计数器。多个独立场景给不同 seed 让 ID 不冲突且仍可比较。

### SeededRng

```python
from nanobot.runtime import SeededRng

rng = SeededRng(seed=42)
assert rng.random() == 0.6394267984578837   # 取决于 CPython 版本
```

跨 Python 实现可能产生不同序列；锁解释器版本以确保跨开发机一致。

### make_stub_handle

[handle.py:137-157](../../nanobot/core/handle.py#L137-L157)：

```python
from nanobot.contracts.ids import RefId
from nanobot.core.handle import make_stub_handle

handle = make_stub_handle(
    RefId("test_001"),
    kind="my.thing",
    target=some_object,                   # 默认 object()
    attributes={"size_kb": 128},
)
handle.attach_to(scope)
```

让你不需要构造完整 `RefDescriptor` 与 finalizer 就能验证：

- handle 生命周期（`acquire` / `release` / `borrow`）
- scope 自动回收
- 故意泄漏触发 `HandleLeakError`

### InMemoryAdapter

[adapters/inmemory.py](../../nanobot/adapters/inmemory.py)：

```python
from nanobot.adapters import InMemoryAdapter

adapter = InMemoryAdapter(channel="test", user="alice")

# 注入入站
await adapter.send_text(agent, "echo hello")

# 等出站
msgs = await adapter.drain_outbox(agent, timeout=0.5)
```

`drain_outbox` 在 `timeout` 内尽量多取消息，遇到第一次 50ms 内无消息时返回（[inmemory.py:51-62](../../nanobot/adapters/inmemory.py#L51-L62)）。

## 标准测试模板

```python
import pytest
from nanobot.adapters import InMemoryAdapter
from nanobot.contracts.ids import AgentId
from nanobot.contracts.lifecycle import LifecyclePhase
from nanobot.core.agent import Agent
from nanobot.core.loader import PluginLoader
from nanobot.runtime import DeterministicIdGen, ManualClock, SeededRng
from nanobot.runtime.scheduler import AgentScheduler

from my_plugin import MyPlugin


@pytest.mark.asyncio
async def test_my_plugin_full_lifecycle():
    clock = ManualClock(start=1_700_000_000.0)
    agent = Agent(
        agent_id=AgentId("test-agent"),
        clock=clock,
        id_gen=DeterministicIdGen(seed=0),
        rng=SeededRng(seed=0),
    )

    loader = PluginLoader(allow={MyPlugin.id})
    await loader.load_into(agent, [MyPlugin])
    scheduler = AgentScheduler(agent)
    await scheduler.start()

    adapter = InMemoryAdapter()
    await adapter.send_text(agent, "mycommand arg1 arg2")
    msgs = await adapter.drain_outbox(agent, timeout=0.5)
    assert "expected" in msgs[0].text

    await scheduler.stop()
    await loader.unload_from(agent)
    assert agent.phase == LifecyclePhase.STOP
```

`tests/plugins/test_echo.py` 与 `tests/runtime/test_scheduler.py` 是两个完整对照样本。

## 常见模式

### 验证错误码

```python
await adapter.send_text(agent, "nonexistent_command")
msgs = await adapter.drain_outbox(agent, timeout=0.5)
assert "[error capability.not_declared]" in msgs[0].text
```

### 收集 trace span

```python
spans = []

async def collect(payload: object) -> None:
    spans.append(payload)

unsub = agent.bus.subscribe("trace.span", collect)
try:
    await adapter.send_text(agent, "echo hi")
    await adapter.drain_outbox(agent, timeout=0.5)
finally:
    unsub()

assert len(spans) == 1
assert spans[0].name.startswith("plugin.")
assert spans[0].status == SpanStatus.OK
```

### 验证泄漏

```python
from nanobot.core.scope import HandleLeakError

class LeakyPlugin(Plugin[Cfg]):
    id = "leaky"
    ...
    async def on_load(self) -> None:
        h = make_stub_handle(RefId(self.agent.id_gen.next("ref")))
        h.attach_to(self.scope)
        _ = h.acquire()      # 故意 +1 不 -1

await loader.load_into(agent, [LeakyPlugin])
with pytest.raises(HandleLeakError) as exc_info:
    await loader.unload_from(agent)
assert exc_info.value.error.code == Errs.HANDLE_LEAK
```

### 验证热重载多次

```python
for _ in range(100):
    await loader.load_into(agent, [MyPlugin])
    await loader.unload_from(agent)
# 不抛 HandleLeakError 即通过
```

`tests/plugins/test_echo.py` 里有这个模式的真实样例。

## 常见陷阱

- **`pytest-asyncio` 模式**——pyproject 已设 `asyncio_mode = "auto"`，所有 async test 自动跑，不必加 `@pytest.mark.asyncio`（但加了也没事）。
- **`ManualClock` teardown 必须 `cancel_all`**——否则 pending sleeper 在 event loop 关闭时会让 pytest 报 "Task was destroyed but it is pending"。最好放进 `pytest.fixture` 的 teardown。
- **`adapter.drain_outbox(timeout=0.5)` 不能太短**——scheduler 的 inbox `wait_for(timeout=0.1)`（[scheduler.py:70](../../nanobot/runtime/scheduler.py#L70)），命令一个完整往返大概要 100ms+。timeout 给 0.5s 是安全值。
- **`PluginRegistry` 是进程级**——多个测试装载同一个 plugin 类要先 unload，否则 `RegistryConflictError`。模板里 `await loader.unload_from(agent)` 必须在 teardown 里跑。
- **`SeededRng` 跨 Python 版本不稳**——CI 里如果 Python 版本变化，"完全相同的随机数序列"断言会失败。建议断言性质（"分布在 0-1 之间"）而不是具体值。
- **`make_stub_handle` 的 target 是 `object()` 默认值**——它什么也不做。测试 `handle.acquire()` 拿到的是这个 object，能验证生命周期但不能验证业务逻辑。要测业务，传一个真 target 进去。
