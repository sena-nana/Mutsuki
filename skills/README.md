# Mutsuki 技能目录

技能按当前架构分组，入口统一为每个目录中的 `SKILL.md`：

- `protocol/`：跨 crate、Host、plugin、runner 和语言边界的公开契约。
- `runtime/`：Core 调度、资源状态、effect、LoadPlan 和 reload。
- `composition/`：PluginScope、scoped service、backend ownership 和 staged lifecycle。
- `sdk/`：Rust SDK、宏、Runner helper 和通用执行适配。
- `governance/`：monorepo package 边界、迁移、发布基线和命名规则。

Link、Hosts、Kits、Plugins 和 Products 的领域技能留在各自 package 的 `skills/` 目录，避免把
package-specific 规则提升成全局规则。每个技能只保留当前架构下会改变决策的约束；可选的详细
schema 或长流程放进该技能的 `references/`。
