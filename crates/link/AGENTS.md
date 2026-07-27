# Mutsuki Link 工作规范

本目录拥有领域中立的连接协议、会话、传输、发现、配对、重连和 transport testkit。
它不拥有具体 Host、业务协议、产品生命周期或 Core 调度。

## 技能路由

- `skills/link-protocol-transport/SKILL.md`：协议、typed envelope、会话、安全和 transport。
- `skills/link-performance/SKILL.md`：时延、公平性、重连和性能模型。

跨 Core 契约先读根 `AGENTS.md` 与 `skills/contracts/SKILL.md`。

## Hard Rules

1. Link 不依赖具体 Host、Agent、Bot 或产品。
2. 控制面和数据面使用稳定 typed contract；协商后热路径不得重复携带可由 session 映射的字段。
3. 认证、会话绑定、frame/payload/queue 预算与重连状态必须结构化验证。
4. transport 只改变传输实现，不改变上层协议语义；不提供生产 fallback 或假连接。
5. 仓内 Mutsuki package 使用 root workspace path；外部协议固定 tag 或 commit。

## 验证

运行 `cargo test -p mutsuki-link --all-features`、Link 相关 workspace tests，以及
`python3 crates/link/scripts/run-performance-model.py --mode smoke`。
