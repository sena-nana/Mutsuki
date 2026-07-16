# Runtime Wire v1

Runtime Wire v1 把封闭 Core 操作与 transport 分层：

```text
mutsuki-runtime-contracts DTO
        -> mutsuki-runtime-wire typed request / Opcode
        -> typed JSONL debug codec | length-prefixed MessagePack codec
        -> stdio | local IPC | native ABI | MutsukiLink opaque bytes
```

## 单一真源

- schema revision：`mutsuki.runtime.wire/1.1.0`；1.1 将 owner-defined plugin config 与
  可选 plugin surface 纳入统一 `plugin.initialize` typed request/response。
- checked-in artifact：`crates/mutsuki-runtime-wire/schema/runtime-wire-v1.json`。
- 已发布 Opcode 只能追加且不得复用；method 名只由 Opcode registry 映射。
- request 类型通过 `WireRequest::Response` 在类型层绑定 response。
- additive unknown field 可忽略；required field 删除或含义改变必须提升 protocol major。

## Frame

Binary frame 使用 big-endian：

```text
u32 body_len
u32 magic
u16 protocol_major
u16 protocol_minor
u16 opcode
u16 flags
u64 request_id
u32 payload_len
payload: typed MessagePack map
```

`body_len` 在分配前检查；header 固定 24 bytes。request/response/error/management 由 flags
区分，`request_id` 是非零 `u64`。

## 初始化与限制

任何业务请求之前先用 `InitializeRequest { hello, config? }` 协商 protocol、codec、schema
revision、feature flags、frame/payload/in-flight 上限和 management 能力。ABI guest 在同一
`ProtocolHelloAck` 的可选 plugin surface 中返回 manifest 与 resource provider ids；process
runner 不返回该 surface。不兼容 major、codec、schema、必需 feature、management 能力或
扩大的限制立即结构化失败。

默认上限：8 MiB frame/JSONL line、4 MiB payload、64 in-flight，其中 8 个槽只供
management。资源 bytes 的 inline 上限为 64 KiB；更大内容必须通过 `ResourceRef`、stream
或 shared descriptor。

## JSONL 兼容边界

Typed JSONL 保留为 debug、conformance 和迁移 codec，不再向调用层公开
`method: &str + Value`。Host reader 与 writer 分离，pending table 按 `u64 request_id`
关联乱序响应；EOF、malformed、oversized、duplicate/late response 会收敛所有 waiter。

JSONL 的淘汰条件是 active release set 全部支持 binary codec 且诊断工具可解析 binary
frame；在此之前它不作为大 batch、大 payload 或高频生产热路径的推荐 codec。
