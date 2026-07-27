---
name: capability-boundaries
description: Route WebHost capabilities to the correct owner repository and keep WebHost at the same layer as TauriHost.
---

# Capability Boundaries

WebHost 是运行宿主，不是业务后台。

## 归属

- WebHost：生命周期、HTTP/WS、静态资源、bridge、extension loader、recovery。
- WebApplication（产品仓库）：Shell、布局、默认扩展组合、品牌与权限策略。
- WebExtension（插件仓库）：页面、导航、slot、renderer、命令与订阅。
- BotPlugins：Schema-first 配置与默认 Web 配置插件（不在本仓库实现业务表单）。
- Core：仅通用协议缺口；禁止下沉 Web 业务类型。
- Link：standalone 进程桥接。

## 流程

1. 从 issue 提取能力与验收。
2. 指定唯一 owner。
3. 上游先实现并验证，再更新本仓库 pin/装配。
4. 禁止在本仓库复制业务页面或添加生产 fallback/shim。
