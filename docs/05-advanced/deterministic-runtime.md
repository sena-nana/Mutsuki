# 确定性运行时与可重放

## 这是什么

MutsukiBot 把时间、ID、随机数都做成**注入式**协议（`Clock` / `IdGen` / `RNG`）。生产环境用 `SystemClock` / `NanoIdGen` / `SeededRng`；测试里用 `ManualClock` / `DeterministicIdGen` / `SeededRng(seed=固定值)`。同一份输入 + 同一份 seed → 同一份 trace。

代码：

- Clock：[mutsukibot/runtime/clock.py](../../mutsukibot/runtime/clock.py)
- IdGen：[mutsukibot/runtime/idgen.py](../../mutsukibot/runtime/idgen.py)
- RNG：[mutsukibot/runtime/rng.py](../../mutsukibot/runtime/rng.py)

## 解决什么问题

[hard rule #9](../../AGENTS.md)：**决定性时间与 ID 由 runtime 注入**。如果不强制：

1. `time.time()` 让 trace 时间无法对齐
2. `uuid.uuid4()` 让"同样的输入"产出"不同的 ID"，回归测试只能匹配字段不能匹配值
3. `random.random()` 让 LLM 采样、抖动、retry 抽样在测试里无法复现

注入的代价：每个插件命令都要拿 `ctx.clock` / `ctx.id_gen` / `ctx.rng`，不能写 `time.time()`。收益：测试可以"按一个输入 → 验证完整 span 序列"，也可以在生产 bug 复现时"导出当时的 seed → 单元测试中重放"。

## 怎么工作

### Clock

[clock.py:13-19](../../mutsukibot/runtime/clock.py#L13-L19) 定义协议：

```python
@runtime_checkable
class Clock(Protocol):
    def now(self) -> float: ...
    def monotonic(self) -> float: ...
    async def sleep(self, seconds: float) -> None: ...
```

#### SystemClock

[clock.py:22-32](../../mutsukibot/runtime/clock.py#L22-L32)：直接转 `time.time` / `time.monotonic` / `asyncio.sleep`。

#### ManualClock

[clock.py:39-96](../../mutsukibot/runtime/clock.py#L39-L96)：墙钟与单调时间只在 `advance(seconds)` 调用时前进。`sleep(s)` 把一个 `(deadline, seq, asyncio.Event)` 三元组放进 min-heap，await event。`advance` 弹出所有 deadline ≤ 当前的 sleeper 并 set 它们的 event。

关键不变量：

- `seq` 是单调计数器，保证同 deadline 时 FIFO 唤醒，且 `(deadline, seq, ...)` 之间永远可比较（避免 heap 用 Event 做 secondary key）
- 推进是 O(k log n)，k 是被唤醒的 sleeper 数
- 超过 `max_pending_waiters`（默认 1024）时发 `ManualClockWaiterOverflow` warning —— 测试很可能漏了 advance

#### cancel_all

[clock.py:90-96](../../mutsukibot/runtime/clock.py#L90-L96)：唤醒所有挂起的 sleeper（用于测试 teardown），返回被唤醒的数量。否则未唤醒的 task 在 event loop 关闭时会报 "Task was destroyed but it is pending"。

### IdGen

[idgen.py:15-19](../../mutsukibot/runtime/idgen.py#L15-L19) 协议：

```python
@runtime_checkable
class IdGen(Protocol):
    def next(self, prefix: str = "") -> str: ...
```

#### NanoIdGen（生产）

[idgen.py:22-36](../../mutsukibot/runtime/idgen.py#L22-L36)：`<prefix>_<26 字符 Crockford-base32>`。前 10 位带时间因子用于粗排序，**不是安全令牌**（`secrets.token_bytes(16)` 只用一半，未充分熵）。

#### DeterministicIdGen（测试）

[idgen.py:39-48](../../mutsukibot/runtime/idgen.py#L39-L48)：

```python
class DeterministicIdGen:
    def __init__(self, seed: int = 0) -> None:
        self._counter = seed

    def next(self, prefix: str = "") -> str:
        self._counter += 1
        body = f"{self._counter:026d}"
        return f"{prefix}_{body}" if prefix else body
```

零填充到 26 位与 NanoIdGen 长度对齐，方便日志对比。`seed` 让你跑两个独立场景仍能区分（`DeterministicIdGen(seed=1000)` vs `DeterministicIdGen(seed=2000)`）。

### RNG

[rng.py:9-15](../../mutsukibot/runtime/rng.py#L9-L15) 协议：

```python
@runtime_checkable
class RNG(Protocol):
    def random(self) -> float: ...
    def randint(self, a: int, b: int) -> int: ...
    def choice(self, seq: list[object]) -> object: ...
```

#### SeededRng

[rng.py:18-31](../../mutsukibot/runtime/rng.py#L18-L31)：薄包装 `random.Random(seed)`。生产 / 测试都用同一类型，区别只在 seed 来源。

### 注入路径

`Agent` 构造时由调用方传入（[agent.py:60-62](../../mutsukibot/core/agent.py#L60-L62)）。Agent 不构造默认值——必须由调用方显式提供。`AgentContext` 把它们 by reference 传给命令（[context.py:38-40](../../mutsukibot/core/context.py#L38-L40)）。

## 用法示例

生产环境：

```python
from mutsukibot.runtime import NanoIdGen, SeededRng, SystemClock

agent = Agent(
    agent_id=AgentId("prod-agent"),
    clock=SystemClock(),
    id_gen=NanoIdGen(),
    rng=SeededRng(seed=42),       # seed 即便在生产也最好显式
)
```

可重放测试：

```python
from mutsukibot.runtime import DeterministicIdGen, ManualClock, SeededRng

clock = ManualClock(start=1_700_000_000.0)
agent = Agent(
    agent_id=AgentId("test-agent"),
    clock=clock,
    id_gen=DeterministicIdGen(seed=0),
    rng=SeededRng(seed=0),
)

# ... 跑业务，期间业务调 ctx.clock.sleep(60) 等 ...
clock.advance(60)                   # 推进 60 秒
clock.advance(60)
# ...

# trace 里的所有 trace_id / span_id 都是确定的，可以做严格断言
```

teardown 时清理 ManualClock 残留 sleeper：

```python
import warnings
with warnings.catch_warnings():
    warnings.simplefilter("error", ManualClockWaiterOverflow)
    # ... 测试 ...
n = clock.cancel_all()
assert n == 0, f"测试结束时还有 {n} 个未唤醒的 sleeper"
```

## 常见陷阱

- **不能 `import time` 直接 `time.time()`**——这是 hard rule #9。v0.1 阶段靠 code review + ruff ASYNC 规则间接拦截，后续会有运行时 lint 规则。
- **`ManualClock.sleep(0)` 直接返回**（[clock.py:64-65](../../mutsukibot/runtime/clock.py#L64-L65)）——不进 heap。
- **`DeterministicIdGen.next("trace")` 与 `next()` 用同一计数器**——前缀不影响计数器自增。所以 trace_001 / trace_002 / span_003 这种序列里 003 的数字是全局递增的。
- **`SeededRng(seed=0)` 跨进程不一定一致**——它依赖 CPython 的 `random.Random`，不同 Python 实现（PyPy 等）可能不同。生产可重放至少要锁解释器版本。
- **`ManualClock` 不会自己推进**——纯粹手动。如果你的代码逻辑期待"等一会儿就好"，但测试代码忘了 `advance`，sleeper 永远不醒，`pytest-asyncio` 可能挂掉 —— 看 `ManualClockWaiterOverflow` 警告就是这种情况。
- **`Clock` 是 `runtime_checkable Protocol`**——`isinstance(x, Clock)` 不严格，只检查方法名存在。要严格类型用 `pyright`。
