# 测试夹具

## 这是什么

MutsukiBot 内置一组用于测试的夹具：`ManualClock` / `DeterministicIdGen` / `SeededRng` / `make_stub_handle` / `InMemoryEndpointPlugin`，以及 v0.2 新增的 dispatcher contract helper、hard rule #14 lint 与 dispatcher invoke benchmark helper。

它们让你能在没有真实平台、没有真实后端、没有真实 wall-clock 的情况下完整复现 Agent 闭环。

## 基础夹具

### ManualClock

```python
from mutsukibot.runtime import ManualClock

clock = ManualClock(start=1_700_000_000.0)
clock.advance(60)
```

### DeterministicIdGen

```python
from mutsukibot.runtime import DeterministicIdGen

gen = DeterministicIdGen(seed=0)
assert gen.next("trace").startswith("trace_")
```

### SeededRng

```python
from mutsukibot.runtime import SeededRng

rng = SeededRng(seed=42)
assert 0 <= rng.random() < 1
```

### make_stub_handle

```python
from mutsukibot.contracts.ids import RefId
from mutsukibot.core.handle import make_stub_handle

handle = make_stub_handle(RefId("test_001"), kind="my.thing", target={"ok": True})
handle.attach_to(scope)
```

## InMemoryEndpointPlugin

`InMemoryEndpointPlugin` 是 v0.2 的进程内 IM transport reference plugin。它注册 `inmemory:default` Source，并提供 `send_text` / `drain_outbox` 两个测试便利方法。

```python
from mutsukibot.contracts import AgentId, Scopes
from mutsukibot.core.agent import Agent
from mutsukibot.core.loader import PluginLoader
from mutsukibot.plugins.inmemory_endpoint import InMemoryEndpointPlugin
from mutsukibot.runtime import DeterministicIdGen, SeededRng, SystemClock

agent = Agent(
    agent_id=AgentId("test-agent"),
    clock=SystemClock(),
    id_gen=DeterministicIdGen(seed=0),
    rng=SeededRng(seed=0),
    accepts=(Scopes.IM_TEXT.to_rule(),),
)
loader = PluginLoader(allow={InMemoryEndpointPlugin.id})
await loader.load_into(agent, [InMemoryEndpointPlugin])

inmem = next(
    p.plugin for p in agent.plugins
    if isinstance(p.plugin, InMemoryEndpointPlugin)
)
await inmem.send_text("echo hello")
msgs = await inmem.drain_outbox(timeout=0.5)
```

## 标准 lifecycle 测试模板

```python
from mutsukibot.contracts import AgentId, Scopes
from mutsukibot.contracts.lifecycle import LifecyclePhase
from mutsukibot.core.agent import Agent
from mutsukibot.core.loader import PluginLoader
from mutsukibot.plugins.inmemory_endpoint import InMemoryEndpointPlugin
from mutsukibot.runtime import DeterministicIdGen, ManualClock, SeededRng
from mutsukibot.runtime.scheduler import AgentScheduler

from my_plugin import MyPlugin


async def test_my_plugin_full_lifecycle():
    agent = Agent(
        agent_id=AgentId("test-agent"),
        clock=ManualClock(start=1_700_000_000.0),
        id_gen=DeterministicIdGen(seed=0),
        rng=SeededRng(seed=0),
        accepts=(Scopes.IM_TEXT.to_rule(),),
    )

    loader = PluginLoader(allow={InMemoryEndpointPlugin.id, MyPlugin.id})
    await loader.load_into(agent, [InMemoryEndpointPlugin, MyPlugin])
    scheduler = AgentScheduler(agent)
    await scheduler.start()

    inmem = next(
        p.plugin for p in agent.plugins
        if isinstance(p.plugin, InMemoryEndpointPlugin)
    )
    await inmem.send_text("mycommand arg1 arg2")
    msgs = await inmem.drain_outbox(timeout=0.5)
    assert "expected" in msgs[0].text

    await scheduler.stop()
    await loader.unload_from(agent)
    assert agent.phase == LifecyclePhase.STOP
```

## v0.2 contract helpers

### Dispatcher 反注册断言

```python
from tests.support.dispatcher_contract import assert_dispatcher_clean_after_unload

await assert_dispatcher_clean_after_unload(
    loader,
    agent,
    operations=("backend:default.notify",),
    sources=("inmemory:default",),
)
```

这个 helper 会卸载插件并断言 dispatcher 中不再残留 Operation / Source。

### Hard Rule #14 lint

```python
from mutsukibot.testing.plugin_lint import lint_plugin_io_fields

violations = lint_plugin_io_fields("mutsukibot/plugins/onebot_v11/__init__.py")
assert violations == []
```

该检查拒绝 `Plugin` 子类字段直接持有 `socket.socket` / `aiohttp.ClientSession` / websocket connection/server 等 raw I/O 对象；应改为 `Handle[T]` 并 attach 到 `PluginScope`。

### dispatcher.invoke benchmark

```python
from mutsukibot.testing.benchmark import measure_dispatcher_invoke

stats = await measure_dispatcher_invoke(
    agent.dispatch,
    "bench:dispatcher.noop",
    ctx=agent.make_context(),
    iterations=200,
)
assert stats.mean_ms < 1.0
```

这个测试是 v0.5+ 延迟敏感链路的早期基线，当前只防止数量级退化。

## 常见陷阱

- Agent 必须声明 `accepts`，否则 dispatcher 会按 hard rule #13 丢弃 envelope。
- `PluginScope.close()` 会先跑 cleanup，再释放 Handle；raw I/O 资源应通过 scope cleanup 与 Handle 一起托管。
- `ManualClock` teardown 要清掉 pending sleepers，避免 event loop 关闭时留下挂起 task。
- `SeededRng` 不建议断言具体随机序列，跨 Python 实现可能变化。
