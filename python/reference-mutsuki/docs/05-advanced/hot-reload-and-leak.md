# 热重载与泄漏检测

## 这是什么

Mutsuki 把"卸载插件后回到干净状态"做成可验证的契约：所有副作用都要登记到 `PluginScope`，scope.close 时统一回收，未释放的 handle 在 close 阶段被显式报告为泄漏（`Errs.HANDLE_LEAK`）。

代码：

- 卸载流程：[`PluginLoader.unload_from`](../../mutsuki/core/loader.py#L122-L133)
- 泄漏检测：[`PluginScope.close`](../../mutsuki/core/scope.py#L103-L167)
- 100 次循环回归用例：[tests/plugins/test_echo.py](../../tests/plugins/test_echo.py)

## 解决什么问题

[hard rule #4](../../AGENTS.md)：**无副作用热重载**。Yume / mind-sim 一类长寿命 agent 必须能在不重启进程的前提下替换插件 —— 否则没人敢热修。

实现的难点不在卸载流程（按加载反序遍历就行），而在"怎么知道有没有清理干净"。Mutsuki 的解法是把**清理验证**塞进 scope 关闭时：

1. 强制副作用过 scope 登记
2. close 阶段反向跑所有 cleanup
3. 任何 cleanup 异常都不能吞掉
4. handle release 后仍存活即视为泄漏

## 怎么工作

### 卸载链

`PluginLoader.unload_from`（[loader.py:122-133](../../mutsuki/core/loader.py#L122-L133)）：

```python
while agent.plugins:
    entry = agent.plugins.pop()                       # 反序
    try:
        await entry.plugin.on_unload()                # 用户钩子
    finally:
        await entry.scope.close()                      # 强制清理（可能抛 HandleLeakError）
```

`on_unload` 抛错时 scope 仍会被 close（finally）—— 即便用户钩子失败，scope 仍要清理副作用。
Operation / Source 反注册不需要额外 detach hook；它们在注册时已经把 disposer 挂入 `PluginScope`，随 `scope.close()` 自动清理。

### scope.close 的两阶段

详见 [PluginScope](../04-guide/plugin-scope.md)。简化版：

```python
async def close(self) -> None:
    self._state.closed = True
    cleanup_failures = []
    for cleanup in reversed(self._state.cleanups):    # LIFO
        try:
            result = cleanup.fn()
            if inspect.isawaitable(result):
                await result
        except Exception as exc:
            cleanup_failures.append({...})            # 不打断后续

    leaked = []
    for handle in self._state.handles:
        if not handle.is_alive():
            continue
        try:
            handle.release()
        except Exception as exc:
            cleanup_failures.append({...})
        if handle.is_alive():
            leaked.append(handle.ref_id)              # release 后仍活着 = 泄漏

    if leaked or cleanup_failures:
        raise HandleLeakError(leaked, error=...)
```

为什么 cleanup 失败不静默：单步失败如果中断，后面的清理就不会跑 —— 等于换了一种泄漏形式。这里选择"全跑完，把所有失败汇总到 evidence 抛一次"。

### Python reference Agent 自有 fallback scope

Python reference Agent 还有一个独立 scope（`_agent_scope`）专门给 lifespan 钩子用，由 `AgentScheduler.stop` 调用 `close_agent_scope` 关闭（[scheduler.py:65](../../mutsuki/runtime/scheduler.py#L65)）。这条路径目前不参与泄漏统计 —— 但同一套 close 流程，所以行为一致。

### 100 次热重载冒烟

[tests/plugins/test_echo.py](../../tests/plugins/test_echo.py) 里有 `test_hot_reload_no_leaks` 这条用例（参见 [v0.1 报告](../../plans/version-reports/v0.1.md) 的"效果检查"表）：

```
for _ in range(100):
    await loader.load_into(agent, [EchoPlugin])
    await loader.unload_from(agent)
```

要求：100 次反复装卸不抛 `HandleLeakError`、内存不持续增长、dispatcher Operation 表保持空。这是 v0.1 的硬门控。

## 用法示例

完整一轮重载：

```python
from mutsuki.core.loader import PluginLoader
from mutsuki.core.scope import HandleLeakError

loader = PluginLoader(allow={"my-plugin"})
await loader.load_into(agent, [MyPlugin])

# ... 业务跑一阵 ...

try:
    await loader.unload_from(agent)
except HandleLeakError as e:
    # 泄漏不该静默；写日志或拒绝再次装载
    log.error("插件泄漏 %s: %s", e.leaked, e.error.evidence)
    raise
```

定位泄漏（dev 时）：

```python
# 复现一个故意泄漏的插件
class LeakyPlugin(Plugin[_Cfg]):
    id = "leaky"
    ...
    async def on_load(self) -> None:
        h = make_stub_handle(RefId(self.agent.id_gen.next("ref")))
        h.attach_to(self.scope)
        _ = h.acquire()      # 故意不 release
```

卸载时拿到的 `HandleLeakError.error.evidence`：

```
{
    "leaked_count": 1,
    "cleanup_failure_count": 0,
    "leaked_first": "ref_00000000000000000000000001",
}
```

`leaked_first` 给你第一个泄漏的 ref_id；如果 leaked 多个，evidence 只放第一个（约束是 evidence 只接受标量），完整 list 在 `e.leaked` 上。

## 常见陷阱

- **`on_unload` 不该是清理副作用的主力**——副作用应该已经登记到 scope。`on_unload` 主要给"scope 不知道的东西"使用（极少见）。
- **重新装载同一个 Plugin 类时小心 Operation 冲突**。如果上一轮 `scope.close()` 没跑完，dispatcher 里的旧 Operation / Source 可能还没反注册，新实例会在注册时 fail-loud。建议先 `unload_from` 完整跑完再 `load_into`。
- **`HandleLeakError.evidence` 里 `cleanup_failures_json` 是字符串**——是把 list[dict] `json.dumps` 后塞进去的，因为 `Error.evidence` 只接受标量。读的时候 `json.loads` 还原。
- **测试里随机出现的"泄漏"通常是 `add_subscription` 漏了**——`bus.subscribe` 返回的 unsub 闭包没登记到 scope，订阅没卸载，下次发布事件时旧实例的 handler 还在跑。手工 grep `bus.subscribe(` 没跟在 `scope.add_subscription` 附近的位置。
- **不要在 `scope.close` 之后再 await `agent.bus.publish`**——订阅可能已经被反向卸载了。落 trace span 应该发生在 close 之前（如 Python reference scheduler 的 finally 块）。
- **`PluginRegistry` 是类注册表，不是实例表**。卸载实例不会触碰 `PluginRegistry`；卸载只是去掉运行实例，类定义仍可被加载。
