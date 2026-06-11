# ResourceHost

## 这是什么

`ResourceHost` 是当前 runtime 的进程内资源托管服务。它用自己的 `PluginScope` 持有真实资源的 `Handle[T]`，让资源生命周期可以长于某个 plugin 实例。

代码：[resource_host.py](../../mutsukicore/core/resource_host.py)。

## 解决什么问题

v0.2 hard rule #14 要求插件字段不能直接持 socket / SDK client / 连接对象，而是用 `Handle[T]` attach 到 scope。这样能保证 plugin unload 时不泄漏资源，但也意味着 reload 会释放物理连接。`ResourceHost` 给 v0.3 提供一个更高层的所有者：plugin 可以向 host 借用资源，reload plugin 时 host 仍能继续持有底层对象。

## 怎么工作

- `create_handle(...)` 创建 `RefCountedHandle[T]`，并 attach 到 host 自己的 scope。
- plugin 卸载只释放 plugin 自己的状态；只要 host 没关闭，资源仍然活着。
- `declare_capacity(cap, total=N)` 声明某类 capability 的容量。
- `acquire(cap, amount=..., owner=...)` 返回 `ResourceLease`。
- 容量不足时抛 `CapabilityExhaustedError`，其中 `error.code == Errs.CAPABILITY_EXHAUSTED`。
- `close()` 释放所有租约并关闭 host scope。

## 用法示例

```python
from mutsukicore.contracts import CapabilityName, RefId
from mutsukicore.core.resource_host import ResourceHost

cap = CapabilityName.register("example.session", declared_by="example")
host = ResourceHost(owner="example-host")
host.declare_capacity(cap, total=2)

handle = host.create_handle(
    RefId("session-1"),
    target={"conn": "opaque"},
    kind="example.session",
    schema_id_target="example.session",
    schema_version_target="1.0.0",
)

lease = host.acquire(cap, amount=1, owner="my-plugin")
lease.release()
await host.close()
```

## 常见陷阱

- **ResourceHost 不是分布式资源管理器**。当前 runtime 只覆盖单进程对象。
- **租约不是权限系统**。它只做容量计数；权限仍走 `PermissionRule`。
- **不要绕过 Handle**。真实资源仍应通过 `create_handle()` 进入 host scope，而不是裸字段挂在 plugin 上。
