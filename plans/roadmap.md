# NanoBot 路线图

本文件回答：**当前在哪个版本、做什么、不做什么、何时进入下一版本**。

## 当前版本：v0.1 最小可运行骨架

**目标**：第一个可装载、可运行、可被测试的 Agent + 一个回声插件 + 一个 in-memory adapter。**v0.1 已完成；产出报告见 [version-reports/v0.1.md](version-reports/v0.1.md)。**

## 历史版本：v0.0 骨架

**目标**：建立项目宪法、分层、契约草案与规则文档，为后续实现提供唯一事实来源。**不写实现代码**。

### v0.0 范围（In Scope）

| 文件 | 状态 |
|---|---|
| `AGENTS.md` | 项目宪法 + 索引 |
| `README.md` | 一句话定位 |
| `plans/roadmap.md` | 本文件 |
| `plans/architecture.md` | 方向、Agent 一等公民、分层、与 Yume / mind-sim 关系、拆解风险 |
| `plans/engineering.md` | 技术栈、目录、插件模型、横切公约实现层规则、测试基础设施 |
| `plans/contracts.md` | 内部协议草案（核心契约对象骨架） |
| `pyproject.toml` | 最小依赖与工具链配置 |

### v0.0 不做（Out of Scope）

- 任何实现代码（`nanobot/` 目录暂不创建）
- LLM provider 集成
- 任何具体消息平台 adapter（OneBot / QQ / Discord / Telegram 等）
- 持久化层
- Web 控制面板
- 国际化
- 性能基准
- Yume / mind-sim 任何插件的实现

### v0.0 验收标准

任意新协作者读完 `AGENTS.md + plans/*` 能复述：

1. NanoBot 是什么 / 不是什么
2. 与 Koishi / NoneBot / AstrBot 的借鉴边界
3. 与 Yume / mind-sim 的关系
4. Yume / mind-sim 为何能拆插件，以及拆解的风险与对策
5. 下一步做什么（v0.1 范围）

文档自身不包含实现代码或 API 形态描述（避免锁死）。

## 下一版本：v0.1 最小可运行骨架

**目标**：第一个可装载、可运行、可被测试的 Agent + 一个回声插件 + 一个 in-memory adapter。

### v0.1 候选范围

- `nanobot/contracts/` 锁定 v0.1 字段（在 [contracts.md](contracts.md) 草案上加版本字段），含通用 by-ref 协议骨架：`RefPayload[T]` / `Handle[T]` / `RefDescriptor` / `BackpressureChannel[T]` / `Replayability` 声明
- `nanobot/core/`：
  - 注册中心（Agent / Plugin / Service registry）
  - 调度器（最小 `tick` 循环）
  - Context 工厂
  - 服务容器（支持 by-value / by-ref）
  - 插件 DAG 加载器
  - `PluginScope` 与 `TransactionScope`，含 `Handle` 自动释放与泄漏检测
- `nanobot/runtime/`：
  - 决定性时间与 ID 源
  - 事件循环包装
  - 同步点检查
- `nanobot/adapters/`：
  - In-memory adapter（测试基础设施）
- `nanobot/plugins/`：
  - 一个 echo 命令插件（同时是 LLM tool，验证「指令即工具」hard rule）
- `nanobot/observability/`：
  - 结构化 trace 写入器（含因果链）
- `tests/`：
  - 基线契约测试套件
  - echo 插件冒烟测试
  - 热重载测试（验证 `PluginScope` 完整回收）
  - by-ref 协议测试：用 stub `Handle` 验证瞬态引用在 ≥2 插件间通过 `RefPayload` 传递、scope 关闭时自动释放、序列化 / 跨域时正确报错

### v0.1 门控

- 一个 Agent 能 spawn → awake → 处理一条 echo → sleep → stop
- echo 插件能被人类触发，也能作为 LLM tool 被调用
- 热重载 echo 插件 100 次后无资源泄漏
- 所有横切公约 lint 规则就位
- Yume v0.4 的某个 `StimulusEvent → ExpressionDecision` 样本能用 v0.1 契约表达（即使没有 Yume 插件实现，也要能序列化 / 反序列化通过）
- 通用 by-ref 协议自洽：用 stub 引用模拟一条「插件 A 产生 ref → 插件 B 借用 ref → scope 关闭自动释放」链路，全程核心代码不出现任何领域字样

## 后续版本（仅方向，不锁字段）

| 版本 | 主题 |
|---|---|
| v0.2 | 真实 adapter（CLI + 一个 IM 平台）、配置 schema 自动校验 |
| v0.3 | 多 Agent 并发、capability 资源协商、saga 原语 |
| v0.4 | Contract test kit、跨插件因果 trace 完整闭环 |
| v0.5 | 第一个 Yume 插件落地（`nanobot-yume-architecture` + `nanobot-yume-kernel` 文本模式）；门控含「latent / 任意非序列化引用在 ≥2 插件间通过通用 `RefPayload` 协议传递，核心代码与 trace 字段中不出现 `latent` / `tensor` / `gpu` 字样」 |
| v0.6 | LLM 桥接插件（多 Provider）、`nanobot-yume-runtime` 文本推理 |
| v0.7 | `nanobot-yume-evolution` 睡眠插件（事务化） |
| v0.8 | mind-sim 插件首批落地 |
| v0.9 | Web 控制面板插件、配置面板自动生成 |
| v1.0 | 完整 Yume v0.4 行为可在 NanoBot 上复现，文档冻结 |

每个 v0.x 完成时产出 `plans/version-reports/v0.x.md`：方向、完成项、基线、运行检查、效果检查、下版门槛。

## 反向论证（红线）

若任一版本出现以下需求，应**修 NanoBot 契约**而不是把能力塞回 Yume / mind-sim 内部：

- 必须把 latent handle 序列化才能跨插件传
- 必须让全部消息走异步队列
- 必须让 sleep 流程通过松耦合事件链表达
- 必须让插件直接 import 兄弟插件实现模块
- 必须在 `core` 中内置某个业务概念（LLM / 记忆 / 情感等）

这是判定 NanoBot 设计是否还在正轨的指针。

## Plan 同步规则

- 代码即事实，plans 是契约 + 决策。
- 公共契约 / 插件协议 / 生命周期阶段 / 服务接口变化 → 同 PR 内更新 `plans/`。
- plans 保持精简，过期讨论删除，但接口契约与决策必须保留。
