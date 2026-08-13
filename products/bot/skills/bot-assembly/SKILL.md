---
name: bot-assembly
description: Assemble the first-party Mutsuki Bot product from external configuration through plugin selection, RuntimeProfile, RuntimeLoadPlan, ServiceRuntimeBuilder, EventSources, and secret references. Use for Bot startup, configuration, and product composition rather than owner capability implementation.
---

# Bot Assembly

将外部配置确定性地转换为可验证的 Bot 产品装配，产品入口只描述所需能力。

## 配置

- 提交无账号、无凭据的最小 `config/bootstrap.toml`；它只声明 Host identity/directories、secret、插件发现和配置仓库选择。
- 按 CLI、`MUTSUKI_BOOTSTRAP`、仓库 `config/bootstrap.toml` 选择 bootstrap；旧完整产品 TOML 必须拒绝。
- Mutsuki Bot 产品显式选择 SQLite repository plugin、document namespace 和路径；Mutsuki 框架不得内置该选择。
- 产品插件选择、WebExtension 选择和每个 owner 配置由 `ConfigService` 保存到独立 provider document。
- 主配置只保存 secret key；实际值由 Host 从显式引用且被忽略的专用 secret 文件或环境变量注入。
- 零插件配置允许启动为空闲 Runtime；未知字段和显式选择后缺失的 capability、plugin、deployment 或 secret 必须结构化失败。

## 装配

1. 打开 bootstrap 选择的配置仓库，空仓库以 CAS 写一次零 Runtime 插件种子；已有文档不覆盖。
2. 将配置文档解析为 capability、plugin、deployment、binding 和 Host 资源需求；Bot 匹配与顺序只来自 active Flow provider snapshot。
3. 只聚合 owner 公开 factory catalog；产品不得注册自有业务 manifest 或 Runner。
4. 启动前生成并校验 RuntimeProfile/RuntimeLoadPlan；registry freeze 后不得越权注册。
5. 通过 `ServiceRuntimeBuilder` 或当前等价 API 启动真实 `ServiceRuntime`，不创建 BotHost。

QQBot、Agent 和 Provider 只是配置选择与验收场景，不得产生绕过 Bot protocol 或上游公开边界的专用路径。测试合法配置的 load plan，并验证缺失项 fail loud、health 只报告真实组件。
