# Protocol

协议 id 在 `crates/mutsuki-agent-protocol`，SDK marker 在 `crates/mutsuki-agent-sdk/src/protocol.rs`。

## Issue #1 映射

| 草案 | 稳定 id |
| --- | --- |
| plan | `mutsuki.agent/run@1` |
| tool.call | `mutsuki.agent.tool/execute@1` |
| memory.query / write | `mutsuki.agent.memory/query@1` · `.../write@1` |
| llm.complete / stream | `mutsuki.agent.model/generate@1` · `.../stream@1` |
| model HTTP effect | `effect.mutsuki.agent.model/http@1` |
| model effect poll | `mutsuki.agent.model/poll@1` |

另有 context / session / prompt / memory.activate 等 MVP 协议。memory / stream 结果可携带 `ResourceRef` / `ResourceCellRef`。
