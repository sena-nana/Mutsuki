# MutsukiPythonRunnerKit 规约

MutsukiPythonRunnerKit 是 **Python Runner SDK + Runtime Kit**，用于让 Python
插件通过 Mutsuki Runner Link 协议接入 Mutsuki Core。它不是 Host，不是 Core，
也不是 Agent kit。

## 一句话定位

为 Python 插件作者和 Python runner 进程提供 SDK、协议镜像、runner loop glue、
transport/codec、manifest 校验和 conformance 测试。Mutsuki Core 仍然拥有
TaskPool、RunnerRegistry、ResourceManager、StateStore、EventLog、TraceLog、
load-plan 校验和调度事实源。

## Hard Rules

1. **不实现第二套 Core**：不得在 Python 侧新增 TaskPool、scheduler、registry freeze、
   hot reload 判定或 state/event 事实源。
2. **只实现 Runner Link 客户端侧**：Python runner 只能通过 runner step、
   management cancel/dispose、task.call 和 resource broker 协议与 Core 通信。
3. **manifest 是权威声明**：decorator 只提供实现绑定和导出便利；启动前能力声明必须可静态读取和校验。
4. **`ctx.call` 只创建 Core task**：不得本地直调其他插件，也不得绕过 Core 的权限、
   trace、取消和路由。
5. **ResourceRef/ValueRef 是 descriptor**：不得跨边界传 Python object、callable、
   socket、SDK client、数据库连接、真实文件句柄、Rust pointer 或裸内存引用。
6. **副作用必须 task 化**：SDK helper 只能声明 side-effect scope 或发出 `effect.*`
   task，不能把真实外部副作用塞进 pure runner。
7. **结构化错误**：协议失败、manifest 不一致、generation/lease 不匹配和 unsupported
   awaitable 必须 fail-loud，禁止吞异常返回默认值。
8. **保持领域中立**：不得加入 Agent、LLM、记忆、HTTP/FS/IM 真实 provider、插件市场、
   venv 管理器或 Tauri 专用逻辑。

## 命名边界

- `Kit`：本仓库整体，表示 Python SDK + runner glue。
- `SDK`：插件作者 API，例如 `Plugin`、`Context`、`TaskError`、resource helper。
- `Backend`：runner 执行形态，例如 `PythonRunnerBackend`。
- `Bridge`：跨边界传输/编解码，例如 `StdioJsonlBridge`。
- `Protocol` / `contracts`：纯 wire shape mirror。
- `Provider`：只用于未来的具体 resource/effect provider，不得混入 SDK facade。
- 禁止把本仓库组件命名为 `Host`；Host 只属于应用运行环境或 native/Tauri/CLI 容器。

## 推荐阅读顺序

1. `README.md`。
2. `plans/repository-boundary.md`。
3. `plans/runner-link-v1.md`。
4. `src/mutsuki_runner_kit/contracts/`。
5. 相关 runtime/backend/resource 实现和测试。

## 技能路由

- `skills/contract-mirror/SKILL.md`：Rust contracts 的 Python wire mirror。
- `skills/runner-backend/SKILL.md`：PythonRunnerBackend、batch 执行、取消和 task.call。
- `skills/transport-resource/SKILL.md`：JSONL transport、codec 和 resource broker。
- `skills/conformance-testing/SKILL.md`：跨语言 conformance、fixture 和兼容验证。

协议变更先读 contract-mirror；跨多个方向时再读 conformance-testing。

## 依赖规则

- 本仓库必须脱离父目录安装、测试和打包；不得引用兄弟仓库本地路径。
- 跨仓库事实只来自已推送的远端 revision；缺失能力先在 owner 仓库补齐，不复制实现或添加生产 shim。

## 验证

Python 代码变更必须运行：

```powershell
uv run ruff check src tests
uv run pyright src tests
uv run pytest
```

协议、runner backend、resource descriptor 或 bridge 行为变更必须补充功能测试或说明已有
conformance 覆盖点。禁止添加只硬匹配日志、字符串或实现细节的低价值测试。
