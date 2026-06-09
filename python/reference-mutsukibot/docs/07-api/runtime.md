# API · `mutsukibot.runtime`（Python reference）

本页描述 `python/reference-mutsukibot` 中旧 Python reference runtime 的 API：
注入到 `AgentContext` 的运行时来源、兼容调度器和 Python backend 边界。当前根级
事实源是 Rust workspace；Rust core 中没有公开名为 `AgentScheduler` 的类型，
调度 / 路由语义由 `AgentRuntime` 方法与 host tick loop 组合表达。

来源：[mutsukibot/runtime/__init__.py](../../mutsukibot/runtime/__init__.py)。

## 模块地图

| 模块 | 公开符号 |
|---|---|
| [`clock`](#clock) | `Clock` / `SystemClock` / `ManualClock` / `ManualClockWaiterOverflow` |
| [`idgen`](#idgen) | `IdGen` / `NanoIdGen` / `DeterministicIdGen` |
| [`rng`](#rng) | `RNG` / `SeededRng` |
| [`scheduler`](#scheduler) | Python reference `AgentScheduler` |
| [`backend`](#backend) | `StrategyBackend` / `OperationBackend` / `ResourceBackend` / `PythonAgentBackend` |

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

这是 Python reference 层的兼容 `AgentScheduler`，用于驱动旧 Python `Agent` 的
`asyncio` tick 循环。不要把它理解成当前根级 Rust runtime 的主调度器。

```python
class AgentScheduler:
    def __init__(self, agent: Agent)
    async def start(self) -> None
    async def stop(self) -> None
    # 内部：_loop / _handle_message / _emit_result / _emit_error
```

Python reference 行为：

- `start()` —— 先进入非路由准备态，fire `on_awake` 钩子；成功后提交
  `phase=AWAKE` 并起 `_loop` task，失败时保持非路由状态
- `stop()` —— 投递 stop sentinel，优雅等待当前消息处理；超时后才 cancel task，随后 fire `on_sleep` → `phase=STOP` → fire `on_stop`，关 agent fallback scope
- `_loop` —— 直接 `await agent.inbox.get()`；stop sentinel 负责唤醒退出
- `_handle_message`：parse → `dispatch.lookup_operation` → `dispatch.invoke` → outbox；Operation 执行 span 由 dispatcher 统一产出

Rust 对应能力：

- `AgentRuntime.register_agent(...)` 创建 `spawn` 状态 Agent。
- `AgentRuntime.start_agent(...)` 以提交语义执行 `on_awake`、刷新 Operation / Source registry，成功后进入 `awake`。
- `AgentRuntime.publish(...)` 校验已注册 Source，并投递给 `awake + accepts` 命中的 Agent。
- `AgentRuntime.select_accepting(...)` 在已注册 Source 前提下选择 `primary_candidate` winner。
- `AgentRuntime.tick_once(...)` 从 inbox 取 envelope 或执行 `next_step`，记录 input / strategy trace，并发布 strategy emitted envelopes。
- `AgentRuntime.invoke_operation(...)` 通过 `OperationHandlerKey` 间接调用 backend，不保存真实 handler。
- `AgentRuntime.stop_agent(...)` 进入 `sleep`，调用 backend `on_stop` 后进入 `stop`。

`mutsuki-runtime-host` 提供 native in-memory host helper，可在不依赖 Python 的情况下
注册 Source / Operation 并跑通最小 Agent loop；根级验证入口是 `cargo test`。

异常分类：`HandleLeakError` → `HANDLE_LEAK`；`ServiceNotFoundError` → `SERVICE_NOT_FOUND`；`KeyError` → `COMMAND_INVALID_ARGS`；其他 → `COMMAND_EXECUTION_FAILED`。通过 dispatcher 调用的命令体异常会先被 dispatcher 包装为 `OPERATION_HANDLER_RAISED`。

## backend

[backend.py](../../mutsukibot/runtime/backend.py)

Rust / Python 分层第一片的 Python 侧边界。它只暴露可序列化 snapshot 与协议，
不暴露 Python callable、socket、SDK client 或真实 `Handle[T]`。

```python
class OperationHandlerKey(Contract):
    plugin_id: str
    plugin_generation: int
    op_id: str
    handler_id: str

class OperationSnapshot(Contract):
    descriptor: OperationDescriptor
    status: BackendOperationStatus
    key: OperationHandlerKey

class SourceSnapshot(Contract):
    descriptor: SourceDescriptor
    plugin_id: str
    plugin_generation: int

class LeaseToken(Contract):
    token_id: str
    ref_id: RefId
    owner: str

class ResourceSnapshot(Contract):
    descriptor: RefDescriptor
    owner: str
    lease_count: int = 0
```

```python
class StrategyBackend(Protocol):
    def on_awake(self, agent_id: AgentId) -> Awaitable[None]
    def on_input(self, agent_id: AgentId, envelope: Envelope) -> Awaitable[StrategyResult]
    def next_step(self, agent_id: AgentId) -> Awaitable[StrategyResult]
    def on_stop(self, agent_id: AgentId) -> Awaitable[None]

class OperationBackend(Protocol):
    def list_operations(self, agent_id: AgentId) -> tuple[OperationSnapshot, ...]
    def invoke(
        self,
        agent_id: AgentId,
        key: OperationHandlerKey,
        payload: dict[str, Any] | None = None,
    ) -> Awaitable[Any]
    def operation_status(self, agent_id: AgentId, key: OperationHandlerKey) -> BackendOperationStatus

class ResourceBackend(Protocol):
    def register(self, descriptor: RefDescriptor, *, owner: str) -> Awaitable[RefId]
    def acquire(self, ref_id: RefId, *, requester: str) -> Awaitable[LeaseToken]
    def release(self, token: LeaseToken) -> Awaitable[None]
    def list_records(self, owner: str | None = None) -> tuple[ResourceSnapshot, ...]
```

`PythonAgentBackend` 把现有 Python `Agent` / `Dispatcher` 暴露为 backend：

- `list_operations()` 返回 dispatcher snapshot，外部 runtime 只能保存
  `OperationHandlerKey`；snapshot 获取失败也属于 backend 边界错误，必须以
  `BackendInvokeError(Error(code=runtime.backend_failed, ...))` 结构化暴露。
- `invoke()` 通过 `Dispatcher.invoke_with_backend_key(...)` 间接调用 handler。
- `list_operations()` 与 `invoke()` 是 Python → 外部 runtime 的错误归一化边界；
  dispatcher 的 `OperationInvokeError` 会包装为 `BackendInvokeError`，未结构化
  异常映射为 `runtime.backend_failed`。
- plugin reload / unload 后旧 key 会以
  `runtime.backend_generation_mismatch` 结构化失败。

`PythonResourceBackend` 是与 Rust `ResourceGate` 对称的进程内治理 backend：

- 只记录 `RefDescriptor`、owner、`LeaseToken` 和 `lease_count`。
- 不保存真实 `Handle[T]`，不负责 finalizer。
- `LeaseToken` 以 `token_id / ref_id / owner` 三元组整体匹配；unknown `ref_id`、
  stale lease 或 token mismatch 以结构化 `ref.not_found` 失败。

对应 Rust workspace：

- `crates/mutsuki-runtime-contracts`：纯协议与 `ScopeRuleSpec.matches(...)`。
- `crates/mutsuki-runtime-core`：`AgentRuntime`、backend trait、Operation registry、
  `ResourceGate` 与 trace bookkeeping。
