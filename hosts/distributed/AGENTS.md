# Mutsuki Distributed Host 工作规范

本目录拥有可选的分布式 Host sidecar：分布式 contracts、持久 registry、任务放置、内容
本地化、故障恢复、控制客户端和性能模型。它不得把节点或共识语义写入 Core、通用 SDK 或
普通 Host。

## 技能路由

- `skills/distributed-runtime/SKILL.md`：sidecar 边界、放置、控制与零侵入契约。
- `skills/distributed-recovery-performance/SKILL.md`：WAL、恢复、故障注入和性能门禁。

同时读取根 `AGENTS.md` 与 `plans/distributed-zero-intrusion-boundary.md`。

## Hard Rules

1. Core/SDK/普通 Host 保持本地语义；跨节点事实只存在于本目录的显式 contracts 和 adapter。
2. 远程执行必须经过可验证 envelope、idempotency、lease/fencing 和结构化失败。
3. WAL、registry、replica receipt 和 compaction 必须保持崩溃一致性与有界资源。
4. 不宣称未装配的 clustered/HA 能力可用，不添加本地假实现或生产 fallback。
5. Link 和 Service control 通过根 Workspace 公开 package 接入，不复制协议。

## 验证

运行本目录全部 package tests、故障恢复测试、`scripts/run-performance-model.py --mode smoke`
和根分布式零侵入检查。
