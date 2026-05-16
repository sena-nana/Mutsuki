# API · transport endpoints

v0.2 起没有独立 `mutsukibot.adapters` 命名空间。外部协议接入都写成 reference plugin。

## 参考插件

| 模块 | 公开符号 |
|---|---|
| [`mutsukibot.plugins.inmemory_endpoint`](../../mutsukibot/plugins/inmemory_endpoint/__init__.py) | `InMemoryEndpointPlugin` |
| [`mutsukibot.plugins.onebot_v11`](../../mutsukibot/plugins/onebot_v11/__init__.py) | `OneBotV11Plugin` |
| [`mutsukibot.plugins.todo`](../../mutsukibot/plugins/todo/__init__.py) | `TodoPlugin` |

## 说明

- `InMemoryEndpointPlugin` 是测试 / 冒烟基础设施。
- `OneBotV11Plugin` 是首个真实 IM transport reference plugin。
- `TodoPlugin` 是工具型 endpoint 范例。

