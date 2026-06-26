# Mutsuki 架构设计

Mutsuki 的当前根级实现是领域中立的 **TaskPool + Plugin Runner runtime**。
核心公式：

```text
CoreRuntime = TaskPool + RunnerRegistry + RunnerLoop + ResultRouter
            + StateStore + ResourceManager + EventLog + TraceLog
```

## 1. 分层

```text
RuntimeProfile + PluginManifest
  -> external/native resolver
  -> RuntimeLoadPlan / RuntimeLock
  -> CoreRuntime boot
  -> Plugin Runner / Resource Provider / Effect Runner
```

依赖方向：

- `contracts` 只定义 serde 纯协议对象。
- `core` 依赖 `contracts`，只实现 runtime mechanics。
- `host` 依赖 `core + contracts`，提供 native runner host 和 JSONL runner client。
- Python runner kit 镜像 contracts，提供 Python runner host 和 stdio runner server；
  Rust crates 不依赖 Python。

## 2. TaskPool

一切待处理内容都是 `Task`。TaskPool 是唯一待处理事实源，取代早期多队列调度形态。

Task 只承载控制字段、少量不可变 immediate value 和 ref：

- `task_id`
- `kind`
- `priority`
- `ready_at_step`
- `payload`
- `input_refs`
- `expected_versions`
- `correlation_id`
- `idempotency_key`
- `runner_hint`
- `registry_generation`
- `required_surfaces`

调度排序固定为：

```text
ready_at_step asc
priority desc
created_sequence asc
task_id asc
```

## 3. Runner

一切执行、编排和外部操作适配单元都是插件提供的 `Runner`。

```rust
fn step(ctx: RunnerContext, tasks: Vec<Task>) -> RuntimeResult<Vec<RunnerResult>>
```

Runner purity：

- `Pure`：只能返回 task、domain event、state delta、effect request。
- `Committer`：只处理 `core.*` kernel task。
- `Effectful`：只处理 `effect.*` 外部副作用 task。

Core 只根据 descriptor 选择、claim 和调用 runner；不解释业务语义。

## 4. ResultRouter

RunnerResult 不直接修改事实源。ResultRouter 规范化输出：

- `deltas` -> `core.commit` task -> Committer runner -> StateStore。
- `events` -> `core.event.append` task -> EventLog。
- `tasks` -> TaskPool。
- `effects` -> `effect.*` task -> Effectful runner。
- `values` / `resources` -> Resource/Event lineage facts。

因此普通 runner 并行执行不会直接破坏状态一致性。

## 5. ResourceManager

控制面经过 core，数据面不一定经过 core。TaskPool 不搬运大数据本体。

资源和值统一以 descriptor 表达：

- `ValueRef`：小型结构化、可共享、可版本化值。
- `ResourceRef`：大型数据、blob、file-backed mmap、stream 或 provider RPC 资源。
- `StateRef`：跨 task 权威语义状态。

默认规则：

- small immutable value 可以 inline。
- 共享、版本化或跨 ABI/进程数据进入 ValueRef/ResourceRef。
- 共享资源默认 readonly/sealed。
- 修改产生新 ref 或 StateDelta。
- 原地写必须 ExclusiveWriteLease。

## 6. Plugin Loading

插件声明能力，RuntimeProfile 决定组合，resolver 生成确定性 load plan，core 只校验和物化。

Core 不负责：

- 插件扫描、下载、安装。
- 依赖解算和版本选择。
- Python/npm/cargo 依赖管理。
- 运行组合策略。

Core 负责：

- 校验 `RuntimeLoadPlan`。
- 校验 runtime descriptor 不超出 lock 授权。
- 构建并 freeze RunnerRegistry、HandlerBindingRegistry、ResourceProviderRegistry、
  EffectRegistry。
- 记录 registry generation、plugin generation 和 contract fingerprint。

## 7. Hot Reload

热重载使用新 plugin generation，不原地替换对象。

Contract surface 兼容性：

- `Identical`：直接热重载。
- `Additive`：可热重载。
- `Deprecated`：可保留兼容处理，但禁止新增占用。
- `Removed`：必须 zero occupancy。
- `Breaking`：必须 migration、drain 或 restart。

Cancel 通过 PluginHost management channel 投递给原 generation。DisposeBag 负责清理
timer、listener、stream、lease、connection 等插件资源。

Core 提供 `reload_with_runners(new_plan, new_runners)` 用于物化新 generation：
先校验新 descriptor 与 load plan，再创建 shadow registry、比较 live surface
occupancy，并按 running invocation 污染状态处理旧 generation：

- `clean` / `local dirty`：通过原 runner 的 management cancel 投递给原 generation，
  task 回到 ready，并在切换后 rebind 到新 registry generation。
- `polluted` / `unknown dirty`：旧 registry 保留为 draining generation，不接收新
  task，等待 invocation settle；settle 后才执行 DisposeBag。

切换 active generation 后，新 task 使用新 registry；旧 generation 不原地替换，也不在
存在污染/未知 running invocation 时提前 dispose。

## 8. Domain Neutrality

模拟个体不是 runtime 实例，而是上层 Store 中的数据聚合。Rust core 中不得出现
Yume、LLM、IM、MCP、ChatCompletion、OneBot 等领域或产品专用执行分支。
