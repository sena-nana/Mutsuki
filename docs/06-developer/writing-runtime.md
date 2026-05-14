# 自定义运行时（Clock / IdGen / RNG）

## 这是什么

`Clock` / `IdGen` / `RNG` 都是 `runtime_checkable Protocol`，框架自带 `SystemClock` / `NanoIdGen` / `SeededRng`（生产）与 `ManualClock` / `DeterministicIdGen` / `SeededRng(seed=...)`（测试）。下游可以替换实现以接入分布式时钟、Snowflake-like ID、加密随机源等。

代码：

- [mutsukibot/runtime/clock.py](../../mutsukibot/runtime/clock.py)
- [mutsukibot/runtime/idgen.py](../../mutsukibot/runtime/idgen.py)
- [mutsukibot/runtime/rng.py](../../mutsukibot/runtime/rng.py)

## 解决什么问题

注入式协议的好处见 [确定性运行时](../05-advanced/deterministic-runtime.md)。但有些场景内置实现不够：

- 多机部署时希望 `id_gen` 用 Snowflake（含机器位）
- 安全敏感场景希望 `rng` 用 `secrets`
- 时钟需要从 NTP / PTP 拉，不是本地 time

`Protocol` 让你换实现而不改任何业务代码。

## 怎么工作

### 协议定义

| 协议 | 必需方法 |
|---|---|
| `Clock` | `now() -> float`（墙钟，秒）<br>`monotonic() -> float`（单调，秒）<br>`async def sleep(seconds: float) -> None` |
| `IdGen` | `next(prefix: str = "") -> str` |
| `RNG` | `random() -> float`（[0,1)）<br>`randint(a, b) -> int`（含两端）<br>`choice(seq: list[object]) -> object` |

`runtime_checkable` 让 `isinstance(x, Clock)` 能用，但只检查方法名存在 —— 不检查签名。严格类型用 pyright。

### 注入点

`Agent` 构造时三参数为必传（[agent.py:60-62](../../mutsukibot/core/agent.py#L60-L62)）：

```python
@dataclass
class Agent:
    agent_id: AgentId
    clock: "Clock"
    id_gen: "IdGen"
    rng: "RNG"
    ...
```

之后传到每个 `AgentContext`（[context.py:38-40](../../mutsukibot/core/context.py#L38-L40)），插件命令通过 `ctx.clock` / `ctx.id_gen` / `ctx.rng` 访问。

### 内置实现要点

#### SystemClock vs ManualClock

| 属性 | SystemClock | ManualClock |
|---|---|---|
| `now()` | `time.time()` | 手工值 `_wall` |
| `monotonic()` | `time.monotonic()` | 手工值 `_mono` |
| `sleep(s)` | `await asyncio.sleep(s)` | event-driven，由 `advance()` 推进 |

ManualClock 内部用 min-heap `[(deadline, seq, asyncio.Event), ...]`，`advance` 弹出所有 deadline ≤ 当前的 sleeper 并 set。详见 [确定性运行时](../05-advanced/deterministic-runtime.md)。

#### NanoIdGen vs DeterministicIdGen

| 属性 | NanoIdGen | DeterministicIdGen |
|---|---|---|
| 输出 | `<prefix>_<26 字符 base32>` | `<prefix>_<26 位零填充十进制>` |
| 熵源 | `secrets.token_bytes(16)` | 内部计数器 |
| 是否安全令牌 | **否**（带时间因子，仅可粗排序） | 否（确定性） |
| 是否单调 | 大致（带时间） | 严格 |

#### SeededRng

包装 `random.Random(seed)`。生产 / 测试都用同一类型，区别只在 seed 来源（生产可以从环境读，测试固定）。

## 用法示例

### 自定义 IdGen：含机器位的 Snowflake

```python
import time
from mutsukibot.runtime.idgen import IdGen

class SnowflakeIdGen(IdGen):
    def __init__(self, *, machine_id: int) -> None:
        if not 0 <= machine_id < 1024:
            raise ValueError("machine_id 必须在 [0, 1024)")
        self._machine = machine_id
        self._counter = 0
        self._last_ms = 0

    def next(self, prefix: str = "") -> str:
        ms = int(time.time() * 1000)
        if ms == self._last_ms:
            self._counter = (self._counter + 1) & 0xFFF
            if self._counter == 0:
                # 同毫秒内序列号溢出 —— 等到下一毫秒
                while int(time.time() * 1000) == ms:
                    pass
                ms = int(time.time() * 1000)
        else:
            self._counter = 0
            self._last_ms = ms
        snowflake = (ms << 22) | (self._machine << 12) | self._counter
        body = f"{snowflake:022d}"
        return f"{prefix}_{body}" if prefix else body
```

### 自定义 RNG：加密随机

```python
import secrets
from mutsukibot.runtime.rng import RNG

class CryptoRng(RNG):
    def random(self) -> float:
        return secrets.randbits(53) / (1 << 53)

    def randint(self, a: int, b: int) -> int:
        return a + secrets.randbelow(b - a + 1)

    def choice(self, seq: list[object]) -> object:
        return seq[secrets.randbelow(len(seq))]
```

### 使用

```python
from mutsukibot.contracts.ids import AgentId
from mutsukibot.core.agent import Agent
from mutsukibot.runtime import SystemClock

agent = Agent(
    agent_id=AgentId("prod-1"),
    clock=SystemClock(),
    id_gen=SnowflakeIdGen(machine_id=42),
    rng=CryptoRng(),
)
```

业务代码完全不变 —— `ctx.id_gen.next("op")` 自动用 SnowflakeIdGen。

## 常见陷阱

- **`Clock.monotonic` 不要返回 `time.time()`**——单调时间必须只增，墙钟可能回拨。混淆会让 trace 出现负数 duration。
- **自定义 `Clock.sleep` 必须是 async**——同步 `time.sleep` 会阻塞整个 event loop，违反 hard rule #10（同步点显式化）。
- **`IdGen.next` 必须是同步**——Protocol 只声明 sync 方法。需要 IO 的 ID 来源（远程发号器）应该缓存 + 异步预取，`next` 直接返回缓存。
- **`prefix` 约定**：常见前缀 `agent` / `trace` / `span` / `msg` / `op` / `ref`。约定 prefix 后跟 `_`（NanoIdGen / DeterministicIdGen 都遵守）。自定义实现保持一致。
- **`RNG.choice` 类型签名是 `list[object]`**——如果你只接受 tuple 或 generator 会与 builtin `random.choice` 行为分叉，下游可能踩坑。统一接受 list。
- **`Protocol` 不强制实现 ABC 抽象方法**——你的类只要有同名方法就算"实现"了协议。但如果方法签名不匹配，pyright 会标。运行时只 `isinstance` 检查方法存在。
- **不要让 `IdGen` 跨进程共享内部状态**——`NanoIdGen` 的 `_counter` 仅本地用，不防碰撞；`DeterministicIdGen` 跨进程同 seed 会产生重复 ID。要跨进程的 ID 用 SnowflakeIdGen 或 UUID 桥。
