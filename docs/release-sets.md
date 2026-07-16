# Mutsuki Release Set

`releases/mutsuki-0.1-alpha-1.toml` 是当前唯一 active release set，也是 Mutsuki
组合 revision、Runtime Wire schema、语言版本、部署支持范围和 capability maturity 的机器可读
事实源。产品配置、账号和 secret 不属于 release manifest。

## 状态语义

- `candidate`：Core breaking change 或依赖升级的候选组合。必须完成全部下游验证，不能用于主分支发布。
- `active`：所有 required repository revision 已推送，BotTemplate clean build、零插件 smoke、关键 owner
  测试、TauriHost compile/smoke 和 Python conformance 均通过。`releases/` 中必须恰好只有一个 active。
- `unsupported`：没有完整验证证据或 capability maturity 不满足部署要求的组合。不得通过 fallback 冒充可用。

上一 active release 在新 release 激活后保留为 `unsupported` manifest；升级说明必须记录 breaking surface、
迁移顺序和支持窗口。当前 `distributed-clustered-production` 明确 unsupported，local-observable 才属于
active 验证范围。

## 升级流程

1. 复制 active manifest，设置新 release 为 `candidate` 并更新 owner revision。
2. 运行 `python3 scripts/release_set.py --manifest <candidate> sync --workspace-root ..`，按依赖顺序审查、
   验证并推送 owner 仓库；脚本会更新受管 Cargo Git pins 和各仓库 lockfile。
3. 在 BotTemplate 更新直接 pins，运行 `cargo update`，再执行：

   ```text
   python3 -m unittest discover -s scripts -p 'test_*.py'
   python3 scripts/release_set.py --manifest <candidate> validate --root .
   python3 scripts/release_set.py --manifest <candidate> report --workspace-root .. --output target/release-set-report.json
   cargo metadata --locked
   cargo fmt --check
   cargo check
   cargo test
   ```

4. CI 从 manifest 的远端 SHA 创建 clean checkout，重复 Rust、Python 与产品 smoke。报告 artifact 中每个
   repository 都必须为 `ok: true`，BotTemplate 的 Cargo metadata 只能出现 manifest 指定的一个 Core source。
5. 将旧 active 改为 `unsupported`，candidate 改为 `active`，提交升级说明。任何缺失 revision、schema
   不一致、多个 Core source 或失败 smoke 都阻止激活。

`report` 检查每个 revision 在本地 Git 对象中存在、下游 manifest 使用 release set pin，并核对 Python
Runtime Wire mirror 的 Core revision/schema。`materialize` 可在空目录从远端精确重建同一组合，避免依赖兄弟
仓库工作树状态。
