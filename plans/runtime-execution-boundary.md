# Runtime Execution Boundary

当前执行边界是 TaskPool + Runner：

- CoreRuntime 负责 TaskPool、RunnerRegistry、RunnerLoop、ResultRouter、StateStore、
  ResourceManager、EventLog 和 TraceLog。
- RuntimeBootstrapper / 外部语言 runner kit 负责提供 runner 实现、resource provider 和
  effect handler。
- Core 不拥有业务对象、不保存 callable、不解释业务 payload。
- Runner 只通过 `RunnerResult` 返回 task、event、delta 和 effect request。

详见 [architecture.md](architecture.md) 和 [contracts.md](contracts.md)。
