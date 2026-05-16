# API · `mutsukibot.runtime`

注入到 `AgentContext` 的运行时来源：时钟、ID 生成器、RNG、调度器、asyncio 门面。

来源：[mutsukibot/runtime/__init__.py](../../mutsukibot/runtime/__init__.py)。

## 模块地图

| 模块 | 公开符号 |
|---|---|
| [`clock`](#clock) | `Clock` / `SystemClock` / `ManualClock` / `ManualClockWaiterOverflow` |
| [`idgen`](#idgen) | `IdGen` / `NanoIdGen` / `DeterministicIdGen` |
| [`rng`](#rng) | `RNG` / `SeededRng` |
| [`scheduler`](#scheduler) | `AgentScheduler` |
| [`loop`](#loop) | `run` / `gather` / `create_task` / `install_sync_point_guard` |

详见 [确定性运行时](../05-advanced/deterministic-runtime.md) · [写自定义运行时](../06-developer/writing-runtime.md)。

---

## clock

[clock.py](../../mutsukibot/runtime/clock.py)

```python
@runtime_checkable
class Clock(Protocol):
    def now(self) -> float
    def monotonic(self) -> float
    async def sleep(self, seconds: float) -> None

class SystemClock:
    # time.time / time.monotonic / asyncio.sleep

class ManualClock:
    def __init__(self, start: float = 0.0, *, max_pending_waiters: int = 1024)
    def now(self) -> float
    def monotonic(self) -> float
    async def sleep(self, seconds: float) -> None
    def advance(self, seconds: float) -> None
    @property
    def pending_waiters(self) -> int
    def cancel_all(self) -> int

class ManualClockWaiterOverflow(RuntimeWarning):
    """挂起 sleeper 数超阈值时发出"""
```

## idgen

[idgen.py](../../mutsukibot/runtime/idgen.py)

```python
@runtime_checkable
class IdGen(Protocol):
    def next(self, prefix: str = "") -> str

class NanoIdGen:
    """生产：<prefix>_<26 字符 Crockford-base32>"""

class DeterministicIdGen:
    def __init__(self, seed: int = 0)
    """测试：<prefix>_<26 位零填充十进制>"""
```

## rng

[rng.py](../../mutsukibot/runtime/rng.py)

```python
@runtime_checkable
class RNG(Protocol):
    def random(self) -> float
    def randint(self, a: int, b: int) -> int
    def choice(self, seq: list[object]) -> object

class SeededRng:
    def __init__(self, seed: int = 0)
    # 包装 random.Random(seed)
```

## scheduler

[scheduler.py](../../mutsukibot/runtime/scheduler.py)

```python
class AgentScheduler:
    def __init__(self, agent: Agent)
    async def start(self) -> None
    async def stop(self) -> None
    # 内部：_loop / _handle_message / _emit_result / _emit_error
```

行为：

- `start()` —— `phase=AWAKE`，fire `on_awake` 钩子，起 `_loop` task
- `stop()` —— set stop event，cancel task，fire `on_sleep` → `phase=STOP` → fire `on_stop`，关 agent fallback scope
- `_loop` —— 反复 `await asyncio.wait_for(agent.inbox.get(), timeout=0.1)`，超时则 continue（让 stop 信号有机会被检测）
- `_handle_message`：parse → `dispatch.lookup_operation` → `dispatch.invoke` → outbox + trace span

异常分类：`HandleLeakError` → `HANDLE_LEAK`；`ServiceNotFoundError` → `PLUGIN_DEFINITION_ERROR/service_not_found`；`KeyError` → `PLUGIN_DEFINITION_ERROR/missing_arg`；其他 → `PLUGIN_DEFINITION_ERROR/command_raised`。

## loop

[loop.py](../../mutsukibot/runtime/loop.py)

asyncio 薄门面：

```python
def run(coro: Awaitable[T]) -> T                    # asyncio.run
def gather(*coros) -> Awaitable[list[object]]       # asyncio.gather
def create_task(coro: Awaitable[T]) -> Task[T]      # asyncio.create_task
def install_sync_point_guard(callback) -> None      # v0.1 占位
```

`install_sync_point_guard` 是 placeholder —— 完整的 `sys.settrace` based 同步点检测在后续版本落地。
