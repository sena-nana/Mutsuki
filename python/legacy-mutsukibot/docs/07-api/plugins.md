# API · `mutsukibot.plugins`

仓库内置参考插件。

## 模块地图

| 模块 | 公开符号 |
|---|---|
| [`echo`](#echo) | `EchoPlugin`（命令路由 + LLM tool 范例）|
| [`echo.smoke`](#echosmoke) | 端到端冒烟入口 |
| [`inmemory_endpoint`](#inmemory_endpoint) | `InMemoryEndpointPlugin`（测试用 IM endpoint） |
| [`onebot_v11`](#onebot_v11) | `OneBotV11Plugin`（OneBot v11 反向 WS reference plugin） |

详见 [跑通 Echo](../03-quickstart/run-echo.md) · [第一个插件](../03-quickstart/first-plugin.md)。

---

## echo

[__init__.py](../../mutsukibot/plugins/echo/__init__.py)

```python
class _EchoConfig(msgspec.Struct, kw_only=True):
    prefix: str = "echo: "

class EchoPlugin(Plugin[_EchoConfig]):
    id = "mutsukibot-echo"
    version = "0.1.0"
    capabilities = [
        Capability(name=Caps.READ_MESSAGE),
        Capability(name=Caps.SEND_MESSAGE),
    ]
    Config = _EchoConfig

    @command(perms=Perms.PUBLIC)
    async def echo(
        self,
        text: str,
        count: Annotated[int, Arg(ge=1, le=10)] = 1,
    ) -> str
```

通过 `pyproject.toml` 的 `[project.entry-points."mutsukibot.plugins"]` 注册：

```toml
[project.entry-points."mutsukibot.plugins"]
echo = "mutsukibot.plugins.echo:EchoPlugin"
```

## echo.smoke

[smoke.py](../../mutsukibot/plugins/echo/smoke.py)

```python
async def main() -> None: ...

if __name__ == "__main__":
    asyncio.run(main())
```

运行：

```bash
uv run python -m mutsukibot.plugins.echo.smoke
```

完整流程：构造 Agent + JsonlTraceWriter + 装载 EchoPlugin + 启动 scheduler + 投 "echo hello" + 收响应 + 卸载。trace 落到 `<gettempdir()>/mutsukibot-echo-smoke.jsonl`。

`smoke.py` 同时是 v0.1 门控的运行检查（参见 [v0.1 报告](../../plans/version-reports/v0.1.md)）。

## inmemory_endpoint

[__init__.py](../../mutsukibot/plugins/inmemory_endpoint/__init__.py)

```python
class InMemoryEndpointPlugin(Plugin[_InMemoryConfig]):
    id = "mutsukibot-inmemory-endpoint"
    version = "0.2.0"
    capabilities = [
        Capability(name=Caps.READ_MESSAGE),
        Capability(name=Caps.SEND_MESSAGE),
    ]
    provides_sources = (_INMEMORY_SOURCE,)
    Config = _InMemoryConfig
```

`InMemoryEndpointPlugin` 提供 `send_text` / `drain_outbox`，用于测试与冒烟脚本。

## onebot_v11

[__init__.py](../../mutsukibot/plugins/onebot_v11/__init__.py)

```python
class OneBotV11Plugin(Plugin[_OneBotV11Config]):
    id = "mutsukibot-onebot-v11"
    version = "0.2.0"
    capabilities = [
        Capability(name=Caps.READ_MESSAGE),
        Capability(name=Caps.SEND_MESSAGE),
        Capability(name=Caps.NETWORK_EGRESS),
    ]
    provides_sources = (_ONEBOT_SOURCE,)
    provides_operations = (_OP_SEND_MSG,)
    Config = _OneBotV11Config
```

OneBot v11 的具体 wire 协议只在这个 plugin 内部出现。
