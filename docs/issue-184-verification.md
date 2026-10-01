# Issue #184 验证记录

验证日期：2026-09-14。在 `main` 的基线
`bd5cc0e1a44d2ecc2a00b4cfb76b4999b9ac87c6` 上完成本次实现、精简与验证。
本记录随实现同次提交；性能报告记录提交前构建的基线及 `dirty: true`，不把父 revision 当作新 release。
未创建 tag 或 PR。
环境：macOS 26.6.2 (25G83)、Apple arm64、rustc/cargo 1.98.0、uv 0.11.28。
本次 55 个变更代码/契约文件的 SHA-256 清单见
`target/issue-184-verification/push-source-manifest.json`；清单 SHA-256 为
`6f0f1eec2b91beb491076b99cc20721b2435f5a3991b6e3d87dac00e66b796f4`，
已逐文件验证当前工作区与独立 clone 一致。

本地 dev/check/test 使用 `CARGO_INCREMENTAL=0`，避免增量构建缓存耗尽磁盘。

## 合同与覆盖

- Core 以 provider/ref/resource generation 校验整组 invalidation，幂等移除 hub entry 和 writer；
  open 与新 plan 构造随后返回 `resource.not_found`。同一 outcome 中删除优先于更新，不保留永久 tombstone。
- SQLite 的 TTL、容量回收和显式 delete 在删除事务中采集身份，提交后经统一 outcome 返回；
  snapshot、失败 create、部分失败 batch/saga/补偿仍保留已提交删除。
- 真实 SQLite + Host 测试包含 capability 自删、确定性过期时间、10,000 次创建/回收的 hub/DB
  存活集合比较、重启不可见和不复用 ID。
- 同步屏障覆盖 offloaded worker 超时、actor 响应性、有界排队、executor 容量为 1、
  旧实例请求在途时 staged reload、结果应用后才派发新实例请求、panic 通道关闭与 shutdown drain。
  原生 async 测试覆盖调用方 future 被取消后仍应用失败结果中的 invalidation。
- provider SDK 统一迁移至 `execute`；调用方成功返回类型不变。ABI 新增
  `resource.provider.execute` (0x300c)，wire schema 为 **1.4.0**，两端需一起重建。
  Rust ABI 真动态库测试和 wire golden/conformance 已执行，Python 只镜像 DTO/artifact。

## 持续复查与修复

本轮复查发现并修复了初版通过测试仍遗漏的问题：

- 原生 async `execute` 曾在 executor 准入前、actor 上构造 Future。现在构造和 poll 均受准入及 panic 隔离；容量不足时不会调用 provider。inline panic 同样隔离并关闭该通道。
- Prepared reload 曾丢弃候选 provider，实际一直使用旧路由。现在候选随计划传递，Core 切换成功后原子替换；缺失候选在切换前失败。完整 reload（包括原先 HostRuntimeConfig 注入的 provider）需显式提供所有 active 实例；targeted reload 保留未受影响实例。
- reload 等待 Runner 排空时曾遗漏资源完成后的通道释放，并且不消费 control mailbox。现在各 drain 共用 actor 完成入口和 mailbox 仲裁；新增测试在 Runner 仍阻塞时确认资源结果已应用且后续请求可继续。
- 每个 offloaded/native-async invocation 都保留对应实例直到 actor 应用结束；Concurrent → Ordered 切换等待所有旧请求，默认并发仍保留。测试用两个真实并发请求、容量拒绝和实例 Weak 引用证明该边界。
- 同次 outcome 的删除/更新冲突从逐项全表扫描改为临时 HashSet 索引（预期线性，而非平方级）；新增 10,000 个删除与 receipt 更新的冲突测试。索引不跨 outcome 保留。
- SQLite 容量回收改为单事务删除最旧前缀，消除逐行事务和 provider 端完整候选 ID 列表；新增 10,000 行 bulk rollback、零字节行及精确存活集合测试。
- pending 请求缓存路由/字节计数；计数用 streaming writer，避免完整 JSON 副本。阻塞队列在通道完成前不反复扫描。

SQLite 新增 32 次同文件实例交替 reload 测试，检查实际调用数、首次启动一次 restore、唯一 ID 和 live inventory。
原在途 reload 测试也增加新实例调用次数、零次重复 restore，以及旧实例应用前保活/应用后释放断言。

## 提交前精简

相较复查完成的实现，5 个 Host 生产文件净减少 83 行：offloaded/native-async 共用
invocation 构造与准入记账；LocalResourceClient 复用单 provider 路由校验与 receipt 提取；
reload 按 active provider 单次选择路由，避免完整 map 克隆后多轮筛除。删除无用的路由参数。
不删减生命周期状态或回归测试；无新增依赖和 wire 变化。

## 实际验证

| 命令 | 结果 |
| --- | --- |
| `CARGO_INCREMENTAL=0 cargo test -p mutsuki-plugin-resource-sqlite -p mutsuki-runtime-host -p mutsuki-runtime-wire --lib --locked`（独立 clone） | 202 passed、1 ignored |
| `python3 skills/governance/monorepo-maintenance/scripts/check_workspace.py` | 通过：164 Rust packages，4 个 CI 条件回归及 22 个 performance 测试 |
| `cargo metadata --locked --format-version 1` | 通过 |
| `cargo fmt --all -- --check` | 通过 |
| `CARGO_INCREMENTAL=0 cargo check --workspace --all-targets --locked` | 通过 |
| `CARGO_INCREMENTAL=0 cargo test --workspace --all-targets --locked` | 通过；日志汇总 1,593 passed、0 failed、3 ignored（238 条结果汇总） |
| `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` | 通过 |
| `bash scripts/check-distributed-boundary.sh` | 通过 |
| `cargo bench-smoke` | PASS：60 cases / 62 gates，release time lane，warmup 0 / samples 1 |
| Python 目录：`uv run ruff check src tests` | 通过 |
| Python 目录：`uv run pyright src tests` | 0 errors / warnings |
| Python 目录：`uv run pytest` | 100 passed |
| `git diff --check` | 通过 |

SQLite 新增 Host dev dependency，因此另用 `git clone --no-local` 建立无兄弟仓库的独立 clone，
同步本次修改后执行 workspace checker、locked metadata，以及
`CARGO_INCREMENTAL=0 cargo test -p mutsuki-plugin-resource-sqlite -p mutsuki-runtime-host -p mutsuki-runtime-wire --lib --locked`：
SQLite 24、Host 165、wire 13 通过，Host 1 个 helper ignored。该验证不依赖兄弟仓库路径。

性能产物：[core-smoke-time.json](../target/mutsuki-benchmarks/core-smoke-time.json)。
报告 ID：`core-smoke-time-20260914T075400Z`，0 failed gates。
这是本机 smoke，不能替代固定机器的批准基线比较；本轮性能优化另以数据结构复杂度和 bulk 行为测试验证，没有宣称固定比例提速。

原始本地日志保存在 `target/issue-184-verification/`（构建产物，不进入版本控制）。
第一次全仓构建因磁盘容量失败，使用 `cargo clean --profile dev` 清理构建产物后重跑。
全仓测试另暴露既有 Agent 取消测试访问本机 service 默认目录，现为该 fixture 配置已有临时目录；
针对性重跑及最终全仓测试均通过。

## 边界

- 本次为当前 macOS 平台、workspace 默认 feature 的 all-targets 验证，不代表所有 OS 或全部 feature 组合。
- 未运行 hosted CI、固定机器多进程批准基线性能比较，以及需要真实外部服务/凭据的 smoke。
- 既有 ignored：QQBot 产品凭据 smoke、显式 Chromium executable smoke、process-runner helper。
- retention 保留 create 前触发及 capability 豁免，不提供计时回收或严格瞬时容量上限。
- 超时只结束调用方等待；有副作用的 ordered 请求仍占用通道直到最终结果应用。
  shutdown 会等待所有执行中的资源请求（包括 Concurrent），永久挂起的 provider 可能延长退出；panic 后需重启该执行通道。
