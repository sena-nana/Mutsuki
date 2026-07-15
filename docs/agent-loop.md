# Agent Loop

`mutsuki.agent/run@1` 是普通 orchestration runner，经 batch-first
`AsyncRunnerAdapter` 接入 Core。run 逐步等待 model/tool 子 task，并从
`TaskOutcome::Completed.output` 解码真实 typed result；只有 lifecycle Completed 而没有
业务 output 时结构化失败。model tool call 会形成普通 tool task，tool output 作为 Tool
message 进入下一轮 model context。trace/correlation/session 在子 task 上透传。

`max_steps`、token 与 cost budget 由 run 状态机判断；Host runner deadline 与 cancel 通过
Core `TaskAwait` cascade 终止等待链。最终 callback task 与 final output 在同一
`RunnerResult` 中派生，Core 在 parent terminal wake 前先校验并入池 callback。
