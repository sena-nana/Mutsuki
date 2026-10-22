# MutsukiStdPlugins 工作规范

本目录拥有 Mutsuki 的领域中立标准协议和通用插件实现。Core 提供 runtime 机制，Host
负责装配与生命周期；本目录实现可复用的 config、db、fs、http、observe、resource 和
workflow 能力，不实现 Agent、Bot、产品配置或平台 UI。

## 阅读与技能路由

先读 MutsukiCore 的 `AGENTS.md`、contracts 和当前仓库公开 API，再按方向读取：

- `skills/protocol-surfaces/SKILL.md`：标准协议、DTO、schema 和 manifest surface。
- `skills/resource-state-providers/SKILL.md`：memory/shared-memory、数据库和状态 provider。
- `skills/io-effect-plugins/SKILL.md`：文件、HTTP、权限和其他 effect gateway。
- `skills/workflow-observe-plugins/SKILL.md`：workflow、广播和观测插件。
- `skills/core-conformance/SKILL.md`：Core 接入、batch-first Runner 和跨仓库验收。

## Hard Rules

1. 标准插件只实现通用领域能力，不把机制下沉到 Core，也不吸收 Agent/Bot/Host 业务。
2. 协议 crate 定义纯 wire shape 并拥有协议 ID；插件引用协议 crate 的 ID 常量，不重复声明字符串，
   通过 manifest 和 RunnerDescriptor 声明真实能力。
3. Runner 只走 batch-first `run_batch`，task 操作使用 `TaskHandle`；局部失败不得污染其他 entry。
4. 外部副作用必须由 effectful Runner/Gateway 执行；资源跨边界只传 `ResourceRef`/`ValueRef`。
5. 能力、backend、permission、secret 或 LoadPlan 授权缺失时结构化失败，不做生产 fallback 或 shim。
6. Secret 只由 Host 引用和注入，不进入 manifest、fixture、日志或提交配置。
7. 仓内 Mutsuki 依赖必须继承根 Workspace 的 path；禁止内部 Git pin、仓库外 Cargo `path` 和本地 `[patch]`。
8. `mutsuki-plugin-*` 名与 `plugins/` 目录都只给真正可加载的插件面，即产出 `PluginManifest`
   的 crate。ConfigRepository backend、WebExtension 这类支撑面放 `crates/`，并按其归属命名
   （如 `mutsuki-config-sqlite`、`mutsuki-std-web-extension-config`）。反向占用前缀会让
   「manifest 与真实能力一致」这条无法靠名字自查——`mutsuki-plugin-config-sqlite` 与
   `mutsuki-plugin-config-web` 正是这样漂移过来的。
   例外只有 `mutsuki-plugin-api` 与 `mutsuki-plugin-host`：它们命名的是插件 ABI 契约与加载器
   本身，不是某个插件。

## 验证

Rust 改动运行 `cargo fmt --check`、`cargo check` 和 `cargo test`。协议、provider、effect 或
LoadPlan surface 改动补充行为测试，并报告实际命令与结果。

## Descriptor 失效（#184）

Provider `execute` 返回 `ResourceProviderOutcome`：已提交的 invalidation 与操作是否成功无关，
batch/saga 部分失败也一样。Host 在 actor 上应用 provider/ref/resource-generation 删除后才回复。
不存在的 ref 幂等；owner 或 generation 冲突则失败。同一 outcome 中 invalidation 优先于更新，并移除
writer/派生 occupancy 事实。回执状态和业务 JSON 不是生命周期信号。

发出 invalidation 的 provider 声明 Ordered。其 provider-id lane 跨越分阶段 reload 保留，执行中的
provider 保留到 actor 应用结果为止；调用方超时或断开后 lane 仍占用，直到调用实际完成。队列条数与
字节使用 Host 限额；带未知副作用的 panic 会使该 lane 失效直到重启。open 中不保留永久 tombstone
历史，也不做 I/O。SQLite 保持 create-before-insert 的 retention 与 capability 豁免。

容量 retention 以单事务删除最旧资源前缀，避免逐行事务和将所有候选 ID 拉到 provider；回收 ID
随 create outcome 的 invalidations 进入 Host actor。测试同时覆盖 bulk rollback、零字节资源、
capability 豁免，以及同文件 provider 实例交替 reload 的调用/restore 计数。

## 异步资源创建（#182）

SQLite 保持 Offloaded 与 Ordered。可 await 的 Host registry 创建仍在 actor 上登记；image-render
可以保留文档化的同步 Host bridge。owner 测试用公开 Host client 覆盖 8 MiB blob 的异步 create、
同步 read 与重启。futures dev 依赖须在独立 clone 中验证。见
../../docs/architecture/async-resource-creation.md。
