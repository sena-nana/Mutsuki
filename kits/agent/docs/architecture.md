# Architecture

MutsukiAgentKit 是 Core 之上的高层 Agent 能力库，不是 Host 补丁。

```text
Product Host / platform adapters
  -> AgentKit (protocols · SDK · plugins)
  -> StdPlugins
  -> MutsukiCore (batch-first TaskPool)
```

Runtime 路径：orchestration runner 通过 `TaskAwaitRunnerAdapter` 等待子 task；外部 model future 由 Host 通过 `AsyncBatchHandler` 执行。descriptor 声明 batch / payload / resources / ordering / control。

## 红线

- 不复制 scheduler；不接管语言 event loop
- memory / stream / LLM output 用 `ResourceRef` / `ResourceCellRef`
- LLM / 工具副作用走 effectful runner 或标准 plugin，不本地直调绕过 TaskPool
- 只依赖 MutsukiCore runtime contracts/SDK，不依赖产品 Host
- 不读取配置文件、环境变量或 Secret backend；产品装配层显式构造并注入服务
- 不声明 Core 内建 Agent 能力

## 非目标

不实现 Core workflow / actor / TaskGroup；不把商业 LLM 集成声明为 Core 功能。
