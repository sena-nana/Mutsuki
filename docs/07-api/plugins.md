# API · `nanobot.plugins`

仓库内置参考插件。

## 模块地图

| 模块 | 公开符号 |
|---|---|
| [`echo`](#echo) | `EchoPlugin`（命令路由 + LLM tool 范例）|
| [`echo.smoke`](#echosmoke) | 端到端冒烟入口 |

详见 [跑通 Echo](../03-quickstart/run-echo.md) · [第一个插件](../03-quickstart/first-plugin.md)。

---

## echo

[__init__.py](../../nanobot/plugins/echo/__init__.py)

```python
class _EchoConfig(msgspec.Struct, kw_only=True):
    prefix: str = "echo: "

class EchoPlugin(Plugin[_EchoConfig]):
    id = "nanobot-echo"
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

通过 `pyproject.toml` 的 `[project.entry-points."nanobot.plugins"]` 注册：

```toml
[project.entry-points."nanobot.plugins"]
echo = "nanobot.plugins.echo:EchoPlugin"
```

## echo.smoke

[smoke.py](../../nanobot/plugins/echo/smoke.py)

```python
async def main() -> None: ...

if __name__ == "__main__":
    asyncio.run(main())
```

运行：

```bash
uv run python -m nanobot.plugins.echo.smoke
```

完整流程：构造 Agent + JsonlTraceWriter + 装载 EchoPlugin + 启动 scheduler + 投 "echo hello" + 收响应 + 卸载。trace 落到 `<gettempdir()>/nanobot-echo-smoke.jsonl`。

`smoke.py` 同时是 v0.1 门控的运行检查（参见 [v0.1 报告](../../plans/version-reports/v0.1.md)）。
