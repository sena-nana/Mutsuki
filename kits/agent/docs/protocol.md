# Protocol

协议 id 在 `crates/mutsuki-agent-contracts`，SDK marker 在 `crates/mutsuki-agent-sdk/src/protocol.rs`。

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

## Session、权限、交互与事件

- `mutsuki.agent.session/create|get|append|snapshot|fork@1` 由 AgentKit Session Runner
  统一拥有；Host 可注入 `SessionPersistence`，但不得解释或复制 transcript、事件序号和
  fork 语义。
- `AgentRunRequest.turn_id` 允许产品绑定稳定 turn；`permission_mode` 为 `ask`、`full` 或
  `read_only`。`full` 仍由 Agent loop 生成版本化批准，`read_only` 将写操作转为结构化
  tool error 后回送模型，不调用目标 effect Runner。
- `AgentToolDescriptor.execution` 区分普通 `routed` 工具和由 Agent loop 拥有的
  `interaction` 工具。Native Coding bundle 提供 `ask_user_question` 与 `confirm_plan`；
  Tool Router 直接执行 interaction descriptor 必须返回 `agent.interaction.loop_required`。
- interaction 工具只能运行在 durable session。模型发出 interaction tool call 后，Run 返回
  `WaitingInteraction` 与版本化 `InteractionRequest`；恢复请求必须复用同一 `session_id`、
  `turn_id`，并提交精确匹配 `{session_id, turn_id, version, interaction_id}` 的
  `InteractionResolution`，不得把回答伪装成新 user turn。
- `InteractionRequest.source_tool`、`permission_mode` 与可选 `context` 由 Agent loop 从已注册
  descriptor 和当前 `AgentRunRequest` 注入，不来自模型参数。产品可据此识别自定义交互并
  校验权限或业务作用域；字段保持可选/默认值，以便旧 checkpoint 和 Wire peer 继续解码。
- 接受后的回答以原 `call_id` 写为 `AgentRole::Tool` 并继续同一 turn；取消写入 error tool
  result 并将 turn 收口为 `Cancelled`。interaction 与 routed tool 不得出现在同一模型批次，
  避免在等待用户期间产生未授权副作用。
- `AgentRunResult.events` 是本次调用产生的单调事件段。持久 session 通过同一次
  `session/append` 原子提交新增消息与事件；事件必须绑定 session 且序号连续。
- `AgentWireAuthority` 位于 Agent Client owner，统一处理 Wire version、idempotency、
  approval/interaction/cancel replay、fork 与 reconnect；产品只实现 `AgentWireRuntime` 和
  持久化注入。Wire peer 必须协商 `interaction-binding` 后才可发送
  `resolve_interaction`；相同 resolution 重放返回已有结果，异值重放返回
  `agent.interaction.idempotency_conflict`。

## Tool 回合因果链

- `AgentToolExecuteResult` 的成功结果携带 `output` 或 `output_ref`；目标工具的业务失败携带
  结构化 `error`，只有工具路由/Runtime 基础设施失败才终止 Agent run。
- Agent loop 将每个结果写为 `AgentRole::Tool`，其 `AgentToolResultMetadata.call_id`
  必须精确引用先前唯一、非空的 `AgentToolCall.call_id`；`is_error` 与 `error` 必须一致。
- Model Adapter 必须保留 assistant tool call 与后续 tool result 的因果顺序。缺失、重复、
  孤立或 malformed call id 是确定性的 protocol error，禁止静默跳过。
- Anthropic Messages 将同一批连续 Tool message 合并为一个 user turn 的多个
  `tool_result` block，并为失败结果设置 `is_error: true`；OpenAI Chat Completions
  路径保留同一 `tool_call_id` 与结构化错误 content；OpenAI Responses 路径使用
  `function_call` / `function_call_output` 项并通过 `call_id` 绑定。

三种基础模型协议 id：`openai.chat-completions`、`openai.responses`、
`anthropic.messages`。详见 [`model-protocols.md`](model-protocols.md) 与
`SimpleReact`。

## Completion 与 Next Edit

- Code Completion 直接调用协议级 Model Adapter，输入 prefix/suffix 与 document version，
  不创建 Agent session，也不执行 Tool。
- Next Edit 输入 `NextEditDocumentContext`、近期编辑、diagnostic、Git diff 与 generation，
  协议模型 planner 只返回版本化 `WorkspaceEditProposal`。
- 可应用的文件改动必须在 `FileChangeDescriptor.edits` 中携带具体
  `WorkspaceTextEdit { range, new_text }`；只有目标或摘要、没有文本编辑的 candidate
  不能作为编辑器 apply/preview 的成功结果。
- 产品在构造原生 WorkspaceEdit 前必须重新执行 candidate version/HEAD 校验；
  AgentKit 不直接修改编辑器工作区。
