# Runtime Wire v1

Runtime Wire 是 Core 唯一的跨语言请求/响应契约。内部传输统一使用 length-prefixed typed
MessagePack frame；Opcode、DTO、semantic fixtures、握手与 P1 request multiplexer 只维护这一套
wire codec。

## Binary frame

所有整数使用网络字节序（big-endian）。帧不依赖 Rust 或 C 结构体内存布局：

| Offset | Width | Field |
| --- | ---: | --- |
| 0 | 4 | body length，不包含自身 |
| 4 | 4 | magic `0x4d555453` (`MUTS`) |
| 8 | 2 | protocol major |
| 10 | 2 | protocol minor |
| 12 | 2 | Opcode |
| 14 | 2 | flags |
| 16 | 8 | non-zero request id |
| 24 | 4 | payload length |
| 28 | N | typed MessagePack payload |

固定 header body 长度为 24 bytes，外加 4-byte length prefix。解码器先读取 prefix，验证
`max_frame_bytes` 后才分配 body；随后验证 payload length 和 `max_payload_bytes`。未知 flag、
Opcode、magic、major、零 request id、截断或长度不一致均关闭连接并使全部 pending 请求失败。

Flags：`0x0001=request`、`0x0002=response`、`0x0004=error`、
`0x0008=management`。request/response 必须且只能出现一个；error 只能用于 response。首版不支持
event flag 和隐式压缩，未知保留位必须拒绝。

## Typed MessagePack

字段名来自 `mutsuki-runtime-wire` 生成 schema，结构体编码为 map。minor 版本可新增带默认值的
字段，旧 decoder 跳过未知字段；删除、改名或改变语义必须提升 major。响应必须匹配 request id、
Opcode 和 `WireRequest::Response`。RuntimeError 只传稳定的 code/source/route/evidence。

payload、frame、inline resource 和 in-flight 数量都有硬上限；MessagePack 编解码最大嵌套深度为
64，单个 array/map 最多 65,536 项，且拒绝顶层值后的尾随字节。大型字节内容必须使用
`ResourceRef`、stream handle 或后续显式共享内存 descriptor，不得
通过提高 ABI 上限绕过资源生命周期。

## Handshake and multiplexing

首个请求必须是 `plugin.initialize`。Hello/Ack 协商 protocol、codec、schema revision、feature
flags、frame/payload/in-flight 上限、management reserved requests 和 management channel。
major、codec、schema 或必要 feature 不兼容时，在业务调用前结构化失败。

stdio binary 与 native ABI v2 都使用 P1 multiplexer 的 u64 correlation、乱序响应、管理预留、
bounded writer queues、timeout 和 fail-all 语义，不得各自维护 pending table。stdout 是协议专用；
日志只能写 stderr 或 Host diagnostics。

## Native ABI compatibility

| ABI | Entry | Bridge | Codec | Status |
| --- | --- | --- | --- | --- |
| v2 | `mutsuki_plugin_abi_v2` | `mutsuki.bridge.abi.binary.v2` | typed MessagePack | production default |
| v1 | `mutsuki_plugin_abi_v1` | `mutsuki.bridge.abi.binary.v1` | typed MessagePack | compatibility entry |

Loader 按 manifest 声明选择 entry，绝不把 v1 symbol 当作 v2。两版 callback 都是
`request(context, ptr, len) -> status + buffer`；返回 buffer 由生产方持有，消费方复制或解码后
必须调用配对 release，context 只 close 一次。null pointer、缺失 callback、错误 transport
version、panic 或非零 status 均跨边界转换为有界错误。
