# TransactionScope 与 Saga

## 这是什么

两套**多步事务**支撑：

- `TransactionScope` —— `PluginScope` 的子类，加上 `commit` / `rollback` 与"补偿动作"概念
- `Saga` —— 一组 `(forward, compensate)` 步骤，主链失败自动反向补偿

代码：

- TransactionScope：[scope.py:170-199](../../mutsukibot/core/scope.py#L170-L199)
- Saga：[mutsukibot/core/saga.py](../../mutsukibot/core/saga.py)

## 解决什么问题

Yume sleep 流程是典型 saga：`collect → evaluate → compile → integrate`。每一步都改 memory / latent 状态；中间失败必须按反向逐步回滚（compile 撤、evaluate 撤、collect 撤），否则下次唤醒看到的就是脏数据。

PluginScope 的 LIFO 清理只解决"插件副作用回收"问题，不解决"业务事务回滚"——业务回滚需要的是按因果反向跑**补偿逻辑**，而不是单纯释放资源。`TransactionScope` 区分这两类，`Saga` 把它编排成串行流水线。

## 怎么工作

### TransactionScope

[scope.py:170-199](../../mutsukibot/core/scope.py#L170-L199)：

```python
class TransactionScope(PluginScope):
    def __init__(self, owner: str) -> None:
        super().__init__(owner)
        self._compensations: list[CleanupFn] = []

    def register_compensation(self, fn: CleanupFn) -> None:
        self._guard()
        self._compensations.append(fn)

    async def commit(self) -> None:
        await self.close()           # 只跑普通 cleanup

    async def rollback(self) -> None:
        for fn in reversed(self._compensations):
            try:
                result = fn()
                if inspect.isawaitable(result):
                    await result
            except Exception:
                continue              # 单个补偿失败不阻塞其余
        await self.close()
```

关键设计：

- 补偿动作**不**在普通 cleanup 列表里——它们是显式的"业务回滚"
- `commit` 不跑补偿，只跑普通 cleanup
- `rollback` 反向跑补偿，再 close
- 补偿失败不抛，被 `close()` 一并报告（如果造成泄漏的话）

### Saga 编排

[saga.py:30-54](../../mutsukibot/core/saga.py#L30-L54)：

```python
@dataclass(slots=True)
class Saga:
    owner: str = "core.saga"
    _steps: list[_Step] = field(default_factory=list)

    def add_step(self, forward: ForwardFn, compensate: CompensateFn, *, name=None) -> None:
        ...

    async def run(self) -> list[Any]:
        results: list[Any] = []
        completed: list[_Step] = []
        try:
            for step in self._steps:
                results.append(await step.forward())
                completed.append(step)
            return results
        except BaseException as exc:
            comp_errors: list[BaseException] = []
            for step in reversed(completed):
                try:
                    await step.compensate()
                except BaseException as ce:
                    comp_errors.append(ce)
            if comp_errors:
                raise SagaCompensationError(exc, comp_errors, error) from exc
            raise
```

关键不变量：

- **只有已完成的步骤会被补偿**——失败的那步不补偿（它"没成功"）
- 补偿按**完成的反序**跑
- 补偿步骤里再失败 → `SagaCompensationError`，同时携带原始错误与补偿错误链

### SagaCompensationError

[saga.py:19-27](../../mutsukibot/core/saga.py#L19-L27)：

```python
class SagaCompensationError(Exception):
    def __init__(self, original: BaseException, comp_errors: list[BaseException], error: Error) -> None:
        super().__init__(...)
        self.original = original
        self.comp_errors = comp_errors
        self.error = error
```

对应错误码：`Errs.TRANSACTION_COMPENSATION_FAILED`。`error.evidence` 至少包含 `owner`、`completed_step_count`、`compensation_failure_count` 与原始异常类型。

## 用法示例

`TransactionScope` 单独使用：

```python
from mutsukibot.core.scope import TransactionScope

@command()
async def transfer(self, ctx: AgentContext, src: str, dst: str, amount: int) -> str:
    txn = TransactionScope(owner=f"transfer.{src}->{dst}")

    debit_done = await self.account.debit(src, amount)
    txn.register_compensation(lambda: self.account.credit(src, amount))

    try:
        await self.account.credit(dst, amount)
    except Exception:
        await txn.rollback()
        raise
    await txn.commit()
    return "ok"
```

用 `Saga` 编排多步：

```python
from mutsukibot.core.saga import Saga, SagaCompensationError

saga = Saga()
saga.add_step(
    forward=lambda: self.collect(input_id),
    compensate=lambda: self.discard_collection(input_id),
)
saga.add_step(
    forward=lambda: self.evaluate(input_id),
    compensate=lambda: self.unevaluate(input_id),
)
saga.add_step(
    forward=lambda: self.compile(input_id),
    compensate=lambda: self.uncompile(input_id),
)

try:
    results = await saga.run()
except SagaCompensationError as e:
    # 补偿本身失败 → 记日志，标 DLQ
    log.error("saga 补偿失败: original=%r, comp=%r", e.original, e.comp_errors)
    raise
except Exception:
    # 主流程失败但补偿成功 —— Saga.run 已经反向跑过补偿
    raise
```

## 常见陷阱

- **`commit` 不跑补偿**——补偿是 "失败时反悔" 的逻辑；commit 是"已经成功，把后续清理跑掉"。如果你想 commit 时也做某事，把它登记成普通 cleanup（`add_subscription` / `add_timer` 等）。
- **`Saga.run` 不接受 trace 上下文**。它本身不 emit span；如果你需要 trace 每一步，要在 forward / compensate 闭包里手工构造子 span（详见 [Trace](../04-guide/trace-and-span.md)）。
- **补偿应当幂等**——`rollback` 时如果某补偿"已经做过一半"就崩了，`Saga` 会跳过它继续。重复运行相同补偿不应有副作用。
- **`Saga` 与 `TransactionScope` 不绑定**——它们是两个独立机制。如果你想要"事务里既有补偿又有 scope 资源"，自己用 `TransactionScope.register_compensation` + 普通 `add_*` 两条路。
- **补偿失败被聚合，不阻断**。这是设计：一个补偿失败不应该阻止后续补偿尝试。如果你需要"严格停止"，在补偿里检测前置条件并 raise —— Saga 仍会继续后续，但你能在 `comp_errors` 里看到。
- **多 saga 嵌套**没有内置语义：外层 saga 的补偿步骤里不会自动卸载内层 saga 已 commit 的事务。手工组合时注意"嵌套保证"靠你自己。
