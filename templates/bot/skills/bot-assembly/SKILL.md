---
name: bot-assembly
description: Assemble Mutsuki BotTemplate from external configuration through plugin selection, RuntimeProfile, RuntimeLoadPlan, ServiceRuntimeBuilder, EventSources, and secret references. Use for startup, configuration, and product composition rather than upstream capability implementation.
---

# Bot Assembly

将外部配置确定性地转换为可验证的 Bot 产品装配，模板只描述所需能力。

## 配置

- 提交一个无账号、无凭据、零插件的简单 `config/template.toml`；不预选平台或业务能力，也不提交完整 Host 高级配置模板。
- 使用被 Git 忽略的 `config/local.toml` 和本地 Secret 文件运行产品；高级目录、IPC、Runner 和观测设置优先继承 ServiceHost 默认，只有产品确有需要时才覆盖。
- 按 CLI、`MUTSUKI_CONFIG`、仓库 `config/local.toml` 选择配置；选中的文件必须存在，不提供无配置 mock/default 模式。
- 用 `[[plugins.configured]]` 声明插件 ID、启用状态和 owner-defined config；模板只注册 factory catalog，不替配置选择插件。
- 主配置只保存 secret key；实际值由 Host 从显式引用且被忽略的专用 secret 文件或环境变量注入。
- 零插件配置允许启动为空闲 Runtime；未知字段和显式选择后缺失的 capability、plugin、deployment 或 secret 必须结构化失败。

## 装配

1. 将配置解析为 capability、plugin、deployment、binding、subscription 和 Host 资源需求。
2. 只聚合上游公开 factory catalog；模板不得注册自有业务 manifest 或 Runner。
3. 启动前生成并校验 RuntimeProfile/RuntimeLoadPlan；registry freeze 后不得越权注册。
4. 通过 `ServiceRuntimeBuilder` 或当前等价 API 启动真实 `ServiceRuntime`，不创建 BotHost。

QQBot、Agent 和 Provider 只是配置选择与验收场景，不得产生绕过 Bot protocol 或上游公开边界的专用路径。测试合法配置的 load plan，并验证缺失项 fail loud、health 只报告真实组件。
