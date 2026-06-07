# API · transport endpoints

v0.2 起没有独立 `mutsukibot.adapters` 命名空间。外部协议接入都写成 reference plugin。

## 参考插件

| 模块 | 公开符号 |
|---|---|
| [`mutsukibot.plugins.inmemory_endpoint`](../../mutsukibot/plugins/inmemory_endpoint/__init__.py) | `InMemoryEndpointPlugin` |
| [`mutsukibot.plugins.onebot_v11`](../../mutsukibot/plugins/onebot_v11/__init__.py) | `OneBotV11Plugin` |

## 说明

- `InMemoryEndpointPlugin` 是测试 / 冒烟基础设施。
- `OneBotV11Plugin` 是首个真实 IM transport reference plugin。
- 非 IM 的外部后端事件由 bridge / 领域插件自定义 `SourceKindName`、`Envelope`
  与 `Operation`；Core 不内置应用后端 endpoint。
