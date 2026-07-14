# 通用执行策略、能力与固定开销画像契约

本文冻结单机 Host 可直接使用的执行策略、能力匹配、实现变体和性能画像。类型只描述本地
执行选择所需事实，不描述部署拓扑、传输成本、资源位置或全局调度。

## 可选接入与旧插件

`ExecutionVariantCatalog` 是独立可选 catalog，不增加 `Task`、`RunnerDescriptor`、
`PluginManifest`、load plan 或 execute ABI 的必填字段。旧插件/旧 Host 不创建也不调用
catalog，继续使用既有 runner binding 和本地 scheduler 行为。

声明 catalog 的插件只描述逻辑 task type、runner/plugin identity、implementation version、
requirements、quality 和 preference，不选择执行机器，也不声明外部调度策略。

## 显式执行策略

`ExecutionPolicy` 包含：

- `LatencyClass`：HardRealtime、SoftRealtime、Interactive、Batch、Background；
- 相对 deadline 与最大排队时间；
- `FailureMode` 和 `NoPlacementPolicy`：Fail、Wait 或显式 Fallback；
- `QualityPolicy`：requested/minimum level，只有 minimum 低于 requested 才允许降质；
- `CachePolicy`：Disabled、FreshOnly 或带 max age 的 AllowStale；
- `PartialResultPolicy`：Reject 或 Allow；
- 本地 `criticality`、base priority、preemptible、pausable 提示。

`select_local` 只在调用方提供的本地 `CapabilitySet` 中做确定性匹配。首选 variant 不可用、
质量下降时，`ExecutionOutcomeMetadata` 必须记录 fallback、实际质量和 degradation reason；
partial/stale 默认 false，只有上层按显式策略使用结果时才能设为 true。严格策略不得静默选择
低质量实现。

HardRealtime/SafetyCritical 等值为单机 Host 保留高于 Background/Deferrable 的表达能力；
Core 不把它扩展为跨机器 fair-share、稀缺度评分或传输成本。

## Capability、Requirement 与 Variant

`CapabilitySet` / `RequirementSet` 使用确定性 set/map 描述：

- architecture 与 instruction set；
- compute backend（如 CPU、CUDA、Metal、Vulkan）；
- precision 与 memory class/bytes；
- runner/plugin version；
- owner-defined custom capability。

`RequirementSet::is_satisfied_by` 只比较调用方传入的本地事实：集合必须覆盖、内存不得低于
minimum、版本必须精确匹配。`ExecutionVariant` 可由同一 task type 声明 CUDA FP16、Metal
FP16、CPU INT8 等实现。descriptor 不含位置或连接信息。

## 固定开销性能画像

`ExecutionProfileAccumulator` 由 Host 显式调用 `record`，内部只有：

- 32 项覆盖式 `FixedSampleWindow`，用于有界 p50/p95/p99；
- 16 个固定 latency upper bounds 与 17 个 `FixedHistogram` 计数槽；
- 一个 `FixedEwma` throughput 值；
- peak memory、failure count、sample count 标量。

记录次数增加不会扩大样本存储。`ExecutionProfile` snapshot 包含 task type、variant、input
bucket、p50/p95/p99、throughput、peak memory、failure rate 和 sample count。
`ProfilingBudget` 与 `PressureLevel` 供 Host 限制采样数量、CPU 时间和内存预算。

contracts/Core 不创建 accumulator，不启动线程，不注册 tick hook，不主动采样或上报。
统计关闭时没有运行时路径和后台开销；是否记录一次 task completion 由 Host 配置显式决定。

## 不变量

1. 质量、缓存、partial、stale、wait、fail 和 fallback 均必须来自显式 policy/outcome。
2. variant 只声明 requirements 与实现 identity，不声明执行位置。
3. profile 的原始样本存储有编译期固定上限，禁止追加无界历史。
4. 选择失败返回 `execution.no_variant`，不能伪造成功或静默降质。
