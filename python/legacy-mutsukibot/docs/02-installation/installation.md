# 安装

## 系统要求

- **Python 3.13+**（[pyproject.toml](../../pyproject.toml) 锁定）
- **uv** —— 包与虚拟环境管理工具（推荐；纯 pip 也能用，但 lockfile 走 uv）
- 操作系统：Linux / macOS / Windows 11 都已验证

## 从源码安装

```bash
git clone <repo-url> MutsukiBot
cd MutsukiBot
uv sync --all-extras
```

`uv sync` 会读 [pyproject.toml](../../pyproject.toml) + [uv.lock](../../uv.lock)，建好 `.venv`，装齐运行时与开发依赖。

### 运行时依赖（最小集）

[pyproject.toml](../../pyproject.toml) 里 `dependencies`：

```
msgspec>=0.18           # 契约对象 + 配置 schema
docstring_parser>=0.16  # 命令 docstring → schema 描述
pyyaml>=6.0             # 配置加载（v0.2 起）
```

### 开发依赖

[pyproject.toml](../../pyproject.toml) 里 `[project.optional-dependencies].dev`：

```
pytest>=8.0
pytest-asyncio>=0.23
ruff>=0.6
pyright>=1.1.380
rich>=13.0
```

加上 `[dependency-groups].dev`：

```
pyrefly>=1.0.0
```

## 验证安装

### 跑测试套

```bash
uv run pytest tests/
```

预期：**45 个测试全过**（参见 [v0.1 报告](../../plans/version-reports/v0.1.md) 的运行检查段）。

### 跑 lint 与类型检查

```bash
uv run ruff check mutsukibot tests
uv run pyright mutsukibot tests
uv run pyrefly check
```

三个命令都应该 0 error。CI 必须三者都通过。

### 跑 echo 冒烟

```bash
uv run python -m mutsukibot.plugins.echo.smoke
```

预期输出（来自 [smoke.py](../../mutsukibot/plugins/echo/smoke.py)）：

```
[smoke] agent smoke-agent phase=spawn
[smoke] loaded plugins: ['mutsukibot-echo']
[smoke] phase=awake
[smoke] outbox -> 'echo: hello\n'
[smoke] phase=stop; trace at /tmp/mutsukibot-echo-smoke.jsonl
```

最后一行的 trace 路径里能找到一行 JSON 的 span 记录。

## 不用 uv？

```bash
python3.13 -m venv .venv
source .venv/bin/activate          # Windows: .venv\Scripts\activate
pip install -e ".[dev]"
pytest tests/
python -m mutsukibot.plugins.echo.smoke
```

`-e .` 是 editable install —— 你在 `mutsukibot/` 下改代码立刻生效，不用重装。

## Windows 提示

- 路径分隔符不影响 —— 所有内部路径用 `pathlib.Path` 处理
- 控制台输出可能含中文，PowerShell 默认编码是 UTF-8（Win11 默认设置），不需额外配置
- `JsonlTraceWriter` 用 `Path(gettempdir())`，Windows 上落到 `%TEMP%`（通常是 `C:\Users\<you>\AppData\Local\Temp\`）

## 常见问题

**Q: pip 报 `ERROR: Package 'mutsukibot' requires a different Python: 3.12.x not in '>=3.13'`**

A: MutsukiBot 锁 Python 3.13+。装一个 3.13：

```bash
# pyenv
pyenv install 3.13.0
pyenv local 3.13.0

# uv（推荐）
uv python install 3.13
```

**Q: `uv run pytest` 卡在 ManualClock 测试不动**

A: 这通常是 ManualClock 测试里漏了 `cancel_all()`，pending sleeper 让 event loop 关不掉。检查测试 teardown。详见 [测试夹具](../06-developer/testing-fixtures.md)。

**Q: `mutsukibot.plugins.echo.smoke` 报 `EntryPoint not found`**

A: 仓库里的 entry_points 在 [pyproject.toml](../../pyproject.toml) 里：

```toml
[project.entry-points."mutsukibot.plugins"]
echo = "mutsukibot.plugins.echo:EchoPlugin"
```

如果改了 pyproject.toml 但没重新 `uv sync` / `pip install -e .`，entry_points 不会更新。重新装一遍即可。

**Q: pyright 报 `reportMissingTypeStubs`**

A: 通常是 `docstring_parser` 没有 stub。pyproject 里 `typeCheckingMode = "standard"` 已经放宽，应当不报；如果你切到 strict，需要自己加 `# pyright: ignore` 或 stub。

## 下一步

→ [跑通 Echo](../03-quickstart/run-echo.md)
