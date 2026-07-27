---
name: remote-dependencies
description: Add or update Mutsuki BotTemplate dependencies as independently consumable remote Git revisions. Use for Cargo dependency changes, upstream pin refreshes, lockfiles, or builds that only work with sibling checkouts.
---

# Remote Dependencies

兄弟仓库只用于核查和上游开发，不能成为模板依赖来源。

## 规则

- 允许仓库内 member crate 使用相对 `path`；禁止越出当前 Git 仓库的 `path` 或本地 `[patch]`。
- 跨仓库依赖使用规范 Git URL 和固定 `rev`；同一上游的 crate 使用同一 revision。
- 检查上游 manifest 能否独立解析；若仍引用兄弟目录，先在上游修复并推送。
- 不 pin 未推送的本地 HEAD，不退回本地 path、复制 crate 或提交临时 patch。

## 流程

1. 用 `git status`、`git rev-parse` 和 `git ls-remote` 对齐本地与远端。
2. 在上游运行 `cargo metadata` 和其规定验证，提交并推送所需能力或边界修复。
3. 确认远端 SHA 后更新模板 Git `rev` 和 `Cargo.lock`。
4. 运行 `cargo metadata --locked`、`cargo fmt --check`、`cargo check`、`cargo test`。
5. 在没有兄弟仓库的独立 checkout 重复 metadata、构建和测试。

远端、revision、package 或传递依赖不可用时，指出具体仓库和 manifest 并失败。
