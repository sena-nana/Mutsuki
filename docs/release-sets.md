# Mutsuki Release Set

`releases/mutsuki-0.1-alpha-3.toml` 是当前唯一 active release set，也是 Mutsuki
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
   python3 scripts/release_set.py --manifest <candidate> validate --root .
   python3 scripts/release_set.py --manifest <candidate> report --workspace-root .. --output target/release-set-report.json
   cargo metadata --locked
   cargo fmt --check
   cargo check
   cargo test
   ```

4. CI 从 manifest 的远端 SHA 创建 clean checkout，重复 Rust、Python 与产品 smoke。报告 artifact 中每个
   repository 都必须为 `ok: true`，BotTemplate 的 Cargo metadata 只能出现 manifest 指定的一个 Core source。
5. 将旧 active 改为 `unsupported`，candidate 改为 `active`，再运行默认 active 校验和脚本测试：

   ```text
   python3 -m unittest discover -s scripts -p 'test_*.py'
   python3 scripts/release_set.py validate --root .
   ```

   提交升级说明。任何缺失 revision、schema 不一致、多个 Core source 或失败 smoke 都阻止激活。

`report` 检查每个 revision 在本地 Git 对象中存在、下游 manifest 使用 release set pin，并核对 Python
Runtime Wire mirror 的 Core revision/schema。`materialize` 可在空目录从远端精确重建同一组合，避免依赖兄弟
仓库工作树状态。

## 当前升级记录

`mutsuki-0.1-alpha-3` 完成 Epic #30 Runtime Wire 升级：统一 18 个 typed opcode，加入请求多路复用、
取消与重复/迟到响应失败语义，并以固定帧头 MessagePack 和 native ABI v2 作为生产默认。ServiceHost
按 manifest 精确选择 ABI，动态库加载移出异步执行器；Python kit 使用同一组跨语言 golden vectors。
JSONL 在 Core 记录的迁移窗口内保留为诊断和兼容格式，旧 JSON RPC 不再用于新调用方。

升级顺序为 Core、ServiceHost、StdPlugins，再到 BotPlugins 与其余消费者，最后更新本模板。四阶段
Rust 性能门槛分别 16/16、5/5、11/11、3/3 通过，Python 46/46 通过；远端 materialize 的九个
repository revision 全部一致。部署 maturity 不变：distributed disabled 与 local-observable 继续
active，clustered 继续 candidate，clustered production 继续 unsupported。`mutsuki-0.1-alpha-2`
已转为 unsupported。

`mutsuki-0.1-alpha-2` 仅将 DistributedHost 从 `d418f750` 升级到 `99c0e848`，纳入单 Controller
Clustered MVP 的断线、脉冲、取消安全和内容落盘修复；Core、Link、ServiceHost、插件、桌面与 Python
revision 均保持不变。该升级不改变外部配置 schema 或 capability maturity：distributed disabled 与
local-observable 继续 active，clustered 继续 candidate，clustered production 继续 unsupported。
`mutsuki-0.1-alpha-1` 已转为 unsupported，不再作为产品 pin。

## WebHost 与 release set

`MutsukiWebHost` **当前不单独列入** `releases/*.toml` 的 `[[repositories]]` 条目，原因如下：

- Release set 的 required owner 集合（见 `scripts/release_set.py` 的 `REQUIRED_REPOSITORIES`）覆盖 Core、ServiceHost、Link、StdPlugins、BotPlugins 等产品/runtime 装配面；WebHost 是 **Web 运行宿主库**，由 BotPlugins（Console 扩展）与 BotTemplate（`web.console` 装配）以 Cargo Git pin 间接锁定 revision。
- Active 组合验证路径为 ServiceHost 嵌入式 Console（Template `web_console_smoke`）与 BotPlugins crate 测试。Standalone Console 已通过 MutsukiLink local 控制桥接（`local://mutsuki.servicehost`）实现最小闭环；WebHost 尚未单独纳入 release set，因 QUIC/远程 Link 仍未落地。
- 当 WebHost 成为独立部署面（Standalone + Link 桥接生产可用）或 revision 需要与 BotPlugins 解耦升级时，再新增 `web_host` repository 条目并扩展 `release_set.py` 同步逻辑。

在此之前，WebHost revision 以 BotTemplate / BotPlugins 工作区 `[workspace.dependencies]` 中的 Git `rev` 为事实源；升级 BotPlugins 或 Template pin 时一并核对 `mutsuki-web-host` / `mutsuki-web-protocol` 与 lockfile 一致。
