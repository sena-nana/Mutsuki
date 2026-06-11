# 第一个插件（Python reference）

本 quickstart 面向 `python/reference-mutsukicore` 的旧 Python reference 插件 API。
如果目标是验证当前 Rust core smoke，请以根目录 `cargo test` 和
`mutsukicore-runtime-host` 的 native smoke 覆盖为准，而不是 Python `AgentScheduler`。

## 目标

照着 `EchoPlugin` 写一个 `greet` 插件，覆盖：

- `Config` 配置 schema
- `@command` 装饰
- `Annotated[..., Arg(...)]` 参数约束
- docstring 驱动 schema 描述
- `on_load` / `on_unload` 钩子

跑通后你应该理解：写一个插件 = 写一个继承 `Plugin[Config]` 的类。

## 准备

完成 [跑通 Echo](run-echo.md)。

## 步骤一：拷贝 echo 当起点

在 `mutsukicore/plugins/` 下新建 `greet/__init__.py`（或者放到你自己的 package 里都可以）：

```python
"""Greet 插件 —— 第一个自己的 MutsukiCore 插件。"""

from typing import Annotated, ClassVar

import msgspec

from mutsukicore import Capability, Caps, Perms, Plugin, command
from mutsukicore.contracts import Arg


class _GreetConfig(msgspec.Struct, kw_only=True):
    greeting: str = "你好"
    exclamation: str = "！"


class GreetPlugin(Plugin[_GreetConfig]):
    """问候插件。"""

    id: ClassVar[str] = "mutsukicore-greet"
    version: ClassVar[str] = "0.1.0"
    capabilities: ClassVar[list[Capability]] = [
        Capability(name=Caps.SEND_MESSAGE),
    ]
    Config = _GreetConfig

    @command(perms=Perms.PUBLIC)
    async def greet(
        self,
        name: Annotated[str, Arg(min_length=1, max_length=32)],
        loud: Annotated[bool, Arg(desc="是否大喊")] = False,
    ) -> str:
        """向某人问好。

        Args:
            name: 对方名字。
            loud: 大喊则全大写。
        """
        text = f"{self.config.greeting}, {name}{self.config.exclamation}"
        return text.upper() if loud else text


__all__ = ["GreetPlugin"]
```

## 步骤二：在 smoke 里用它

最快的方式是改一份 smoke 脚本（保留 echo 那份）。在 `mutsukicore/plugins/greet/smoke.py`：

```python
import asyncio
from pathlib import Path
from tempfile import gettempdir

from mutsukicore.contracts import AgentId, Scopes
from mutsukicore.core.agent import Agent
from mutsukicore.core.loader import PluginLoader
from mutsukicore.observability import JsonlTraceWriter
from mutsukicore.plugins.greet import GreetPlugin
from mutsukicore.plugins.inmemory_endpoint import InMemoryEndpointPlugin
from mutsukicore.runtime import NanoIdGen, SeededRng, SystemClock
from mutsukicore.runtime.scheduler import AgentScheduler


async def main() -> None:
    agent = Agent(
        agent_id=AgentId("greet-agent"),
        clock=SystemClock(),
        id_gen=NanoIdGen(),
        rng=SeededRng(seed=0),
        accepts=(Scopes.IM_TEXT.to_rule(),),
    )
    trace_path = Path(gettempdir()) / "greet-smoke.jsonl"
    writer = JsonlTraceWriter(trace_path)
    writer.attach(agent.bus)

    loader = PluginLoader(allow={InMemoryEndpointPlugin.id, GreetPlugin.id})
    await loader.load_into(agent, [InMemoryEndpointPlugin, GreetPlugin])

    scheduler = AgentScheduler(agent)
    await scheduler.start()

    inmem = next(
        p.plugin for p in agent.plugins
        if isinstance(p.plugin, InMemoryEndpointPlugin)
    )
    await inmem.send_text("greet 世界")
    await inmem.send_text("greet world true")
    await asyncio.sleep(0.3)
    msgs = await inmem.drain_outbox(timeout=0.5)
    for m in msgs:
        print(f"-> {m.text!r}")

    await scheduler.stop()
    await loader.unload_from(agent)
    writer.detach()


if __name__ == "__main__":
    asyncio.run(main())
```

跑：

```bash
uv run python -m mutsukicore.plugins.greet.smoke
```

预期：

```
-> '你好, 世界！'
-> '你好, WORLD！'
```

第二行的 `world` 全大写，因为我们在命令里多传了 `true`，被 Python reference
`AgentScheduler` 按 `parameters_schema` 强转成 bool（[scheduler.py:217-218](../../mutsukicore/runtime/scheduler.py#L217-L218)）。

## 步骤三：理解 PluginMeta 在背后做了什么

把这一行加到 smoke 里：

```python
print("manifest:", GreetPlugin.__manifest__)
print("commands:", GreetPlugin.__commands__)
print("source:", GreetPlugin.__source_location__)
```

输出会显示：

```
manifest: PluginManifest(id='mutsukicore-greet', version='0.1.0', capabilities=(...), commands=(CommandSpec(...),), ...)
commands: (CommandSpec(name='greet', description='向某人问好。', ..., parameters_schema={'type': 'object', 'properties': {'name': {'type': 'string', 'minLength': 1, 'maxLength': 32, 'description': '对方名字。'}, 'loud': {'type': 'boolean', 'description': '是否大喊'}}, 'required': ['name']}, ...),)
commands: ...
source: .../mutsukicore/plugins/greet/__init__.py:14
```

`description` 取自 docstring 首段（"向某人问好。"），参数描述按 Google 风格 `Args:` 段抽出，约束（`minLength` / `maxLength`）来自 `Arg(...)`。整套合成由 [`_build_command_spec`](../../mutsukicore/core/plugin.py#L170-L274) 在 class 定义那一刻完成。

详见 [插件定义](../04-guide/plugin-definition.md) 与 [命令与 Schema](../04-guide/command-and-schema.md)。

## 步骤四：加一个 `on_load` 钩子

让插件在装载时订阅 `trace.span` 事件（练习 `PluginScope` 资源回收）：

```python
class GreetPlugin(Plugin[_GreetConfig]):
    id: ClassVar[str] = "mutsukicore-greet"
    version: ClassVar[str] = "0.1.0"
    capabilities: ClassVar[list[Capability]] = [
        Capability(name=Caps.SEND_MESSAGE),
    ]
    Config = _GreetConfig

    async def on_load(self) -> None:
        async def _on_span(payload: object) -> None:
            print(f"  [trace] greet 看到了 span: {payload.name}")
        unsub = self.bus.subscribe("trace.span", _on_span)
        self.scope.add_subscription(unsub)

    @command(perms=Perms.PUBLIC)
    async def greet(self, name: str) -> str:
        ...
```

跑 smoke 时会看到额外输出：

```
  [trace] greet 看到了 span: plugin.mutsukicore-greet.greet
-> '你好, 世界！'
```

`add_subscription` 把 unsub 闭包登记到 scope；卸载时 scope 自动调用它解除订阅。详见 [PluginScope](../04-guide/plugin-scope.md)。

## 步骤五：写一条测试

新建 `tests/plugins/test_greet.py`（或者在你自己的项目里）：

```python
from mutsukicore.contracts import AgentId, Scopes
from mutsukicore.contracts.lifecycle import LifecyclePhase
from mutsukicore.core.agent import Agent
from mutsukicore.core.loader import PluginLoader
from mutsukicore.runtime import DeterministicIdGen, ManualClock, SeededRng
from mutsukicore.runtime.scheduler import AgentScheduler

from mutsukicore.plugins.greet import GreetPlugin
from mutsukicore.plugins.inmemory_endpoint import InMemoryEndpointPlugin


async def test_greet_full_lifecycle() -> None:
    agent = Agent(
        agent_id=AgentId("test-agent"),
        clock=ManualClock(start=1_700_000_000.0),
        id_gen=DeterministicIdGen(seed=0),
        rng=SeededRng(seed=0),
        accepts=(Scopes.IM_TEXT.to_rule(),),
    )

    loader = PluginLoader(allow={InMemoryEndpointPlugin.id, GreetPlugin.id})
    await loader.load_into(agent, [InMemoryEndpointPlugin, GreetPlugin])
    scheduler = AgentScheduler(agent)
    await scheduler.start()

    inmem = next(
        p.plugin for p in agent.plugins
        if isinstance(p.plugin, InMemoryEndpointPlugin)
    )
    await inmem.send_text("greet alice")
    msgs = await inmem.drain_outbox(timeout=0.5)
    assert msgs[0].text == "你好, alice！"

    await scheduler.stop()
    await loader.unload_from(agent)
    assert agent.phase == LifecyclePhase.STOP
```

跑：

```bash
uv run pytest tests/plugins/test_greet.py
```

详见 [测试夹具](../06-developer/testing-fixtures.md)。

## 你刚才学到的

- 插件 = `Plugin[Config]` 子类 + `@command` 装饰的 async 方法
- 元数据（id / version / capabilities / Config）是 `ClassVar`，元类负责校验
- 参数约束写在 `Annotated[..., Arg(...)]`，描述写在 docstring
- 副作用（订阅 / 定时器 / 服务）必须登记到 `self.scope`
- 命令路径是 `Message → Dispatcher.publish → Python reference AgentScheduler → Dispatcher.invoke → solve → outbox + trace`

## 下一步

- 想理解元类做的所有事 → [插件定义与 PluginMeta](../04-guide/plugin-definition.md)
- 想把命令同时给 LLM 用 → [命令与 Schema](../04-guide/command-and-schema.md)
- 想插件之间共享 state → [服务容器](../04-guide/service-container.md)
- 想做事务回滚 → [TransactionScope 与 Saga](../05-advanced/transaction-scope-saga.md)
