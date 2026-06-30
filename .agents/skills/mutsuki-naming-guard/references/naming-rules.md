# Mutsuki Naming Rules

## Core Definitions

| Name | Use only when the component answers |
| --- | --- |
| `Host` | Which application environment Mutsuki runs inside |
| `Backend` | How a plugin or runner is loaded and executed |
| `Bridge` | How two runtime, protocol, process, or UI boundaries connect |
| `Services` | Which external capabilities the host provides to plugins |
| `Protocol` | Which task/resource/event/effect types exist |
| `SDK` | How plugin authors use typed helpers and facade APIs |
| `Plugin` | Which replaceable behavior implementation runs |
| `Core` | The runtime kernel, or a product-domain plugin set |
| `Adapter` | How an external or legacy system is translated into current protocol |
| `Provider` | Which concrete implementation supplies an external capability |
| `Gateway` | Which controlled exit handles permissions, policy, audit, or side effects |
| `Runtime` | A runtime instance or execution framework |
| `Kernel` | The minimal scheduling fact source |

## Naming Table

| Question | Responsibility | Name form | Examples |
| --- | --- | --- | --- |
| Where does Mutsuki run? | Application embedding environment | `*Host` | `MutsukiTauriHost`, `MutsukiCliHost`, `MutsukiServiceHost` |
| How is a plugin loaded/executed? | Deployment/execution form | `*PluginBackend`, `*RunnerBackend` | `NativePluginBackend`, `AbiPluginBackend`, `JsonlRunnerBackend` |
| How is a plugin discovered/loaded? | Loader | `*PluginLoader` | `BuiltinPluginLoader`, `ManifestPluginLoader` |
| How do two boundaries convert? | Boundary connector | `*Bridge` | `TauriEventBridge`, `JsonlBridge`, `ResourceBridge` |
| What can plugins call from the host? | Host capability set | `*Services`, `*ServiceRegistry` | `LiliaAgentServices`, `HostServiceRegistry` |
| Which actions exist? | Type contract | `*Protocol` | `LiliaAgentProtocol`, `MutsukiResourceProtocol` |
| How do plugin authors write code? | Authoring facade | `*SDK` | `LiliaAgentSDK`, `MutsukiRuntimeSDK` |
| What implements behavior? | Replaceable implementation | `*Plugin` | `LiliaClaudePlugin`, `LiliaMemoryPlugin` |
| What is a standard plugin pack? | Product-domain capability set | `*Core` | `LiliaCore`, `YumeCore` |
| How is an external API connected? | External translation or supplier | `*Adapter`, `*Provider` | `CodexAdapter`, `ClaudeProvider` |
| Where do permissions and side effects exit? | Controlled external exit | `*Gateway` | `PermissionGateway`, `EffectGateway` |
| How is data persisted? | Storage abstraction | `*Store`, `*Repository` | `SessionStore`, `TaskRepository` |

## Host

`Host` is only for application runtime environments.

Valid:

- `MutsukiTauriHost`
- `MutsukiCliHost`
- `MutsukiServiceHost`
- `MutsukiTestHost`

Responsibilities:

- Start, stop, and restore `HostRuntime`.
- Own the application lifecycle.
- Provide IPC, event, resource, or effect exits.
- Coordinate the relationship between runtime and external application environment.

Invalid:

- `NativeHost` -> `NativePluginBackend`
- `ABIHost` -> `AbiPluginBackend`
- `JsonlHost` -> `JsonlRunnerBackend` or `JsonlBridge`
- `PythonHost` -> `PythonRunnerBackend` or `PythonRunnerKit`
- `AgentHost` -> `LiliaAgentServices`, `LiliaAgentSDK`, or `LiliaAgentProtocol`

## Backend

Use `Backend` for plugin or runner execution forms:

- `NativePluginBackend`
- `AbiPluginBackend`
- `JsonlRunnerBackend`
- `PythonRunnerBackend`
- `WasmPluginBackend`
- `SidecarRunnerBackend`

Backends may handle runner invocation, codec/encoding, lifecycle, cancel, dispose, and health checks. They must not own Tauri app handles, UI events, timeline, credentials, workspace policy, or user permission policy.

## Bridge

Use `Bridge` for cross-boundary transfer or conversion:

- `TauriEventBridge`
- `TauriIpcBridge`
- `ResourceBridge`
- `EffectBridge`
- `JsonlBridge`
- `RuntimeEventBridge`

A bridge should convert shapes such as `RuntimeEvent -> Tauri event`, `ResourceRef -> blob id`, `EffectRequest -> pending interaction`, or `JSONL line -> RunnerResult`. If it chooses between Claude and Codex, it is a `Policy` or `Router`, not a bridge.

## Services

Use `Services` for host-provided capability sets consumed by plugins:

- `LiliaAgentServices`
- `LiliaWorkspaceServices`
- `HostServiceRegistry`
- `CredentialService`
- `PermissionService`
- `TimelineService`
- `WorkspaceService`
- `BrowserService`

Services may depend on Tauri, SQLite, system APIs, and host-local capability providers. Plugins must depend on service traits or descriptors, not these bottom-layer implementations.

## Protocol

Use `Protocol` only for pure contracts:

- protocol id
- request/response types
- event types
- effect types
- resource kind
- handler binding conventions

Do not include provider API calls, app-server calls, Tauri emit, SQLite writes, UI state, or default execution policy.

## SDK

Use `SDK` for plugin-author helpers:

- typed `ctx.call`
- request builders
- resource handle wrappers
- effect helpers
- descriptor builders
- macros/derives

SDK code must lower to protocol objects, tasks, resource descriptors, plans, and runner results. It must not become a runtime host, scheduler, or execution backend.

## Plugin And Core

Use `Plugin` for replaceable behavior implementations, for example `LiliaClaudePlugin` or `LiliaMemoryPlugin`.

Use `Core` only for:

- `MutsukiCore`: the runtime kernel project.
- `LiliaCore` or another product-domain plugin set.

`LiliaCore` is not a Host, Runtime, or SDK; it is a plugin set.

## Adapter, Provider, Gateway

Use `Adapter` for external or legacy translation, such as `CodexAppServerAdapter` or `LegacyAgentRunnerAdapter`.

Use `Provider` for concrete external capability suppliers, such as `ClaudeProvider`, `CodexProvider`, `OpenAiProvider`, `BrowserProvider`, or a resource provider.

Use `Gateway` for controlled permission and side-effect exits, such as `EffectGateway`, `PermissionGateway`, `ToolExecutionGateway`, `FileWriteGateway`, or `NetworkGateway`.

## Store, Repository, Registry, Cache, State

| Name | Use |
| --- | --- |
| `Store` | Local application/runtime state storage |
| `Repository` | CRUD abstraction for a data type |
| `Registry` | Active object, capability, or session registration |
| `Cache` | Discardable cache |
| `State` | In-memory state object |

## Critical Current Corrections

- `NativeHost` -> `NativePluginBackend`
- `ABIHost` -> `AbiPluginBackend`
- `PythonHost` -> `PythonRunnerBackend` or `PythonRunnerKit`
- `JsonlHost` -> `JsonlRunnerBackend` or `JsonlBridge`
- `TauriHost` -> `MutsukiTauriHost`
- `ServiceHost` -> `MutsukiServiceHost`
- `AgentHost` -> `LiliaAgentServices`, `LiliaAgentSDK`, or `LiliaAgentProtocol`
- `AgentSDK` -> `LiliaAgentSDK`
- `AgentProtocol` -> `LiliaAgentProtocol`
- `LiliaCodeAdapter` -> `LiliaCodeMutsukiAdapter`

## Final Rule

Choose names by the question answered:

1. Scheduling fact source -> `Core` / `Kernel`
2. Application environment -> `Host`
3. Plugin execution form -> `Backend`
4. Plugin discovery/loading -> `Loader`
5. Boundary connector -> `Bridge`
6. Type contract -> `Protocol`
7. Authoring facade -> `SDK`
8. Host capabilities -> `Services` / `Service`
9. Replaceable behavior -> `Plugin`
10. Product plugin set -> `Core`
11. External/legacy translation -> `Adapter`
12. External capability supplier -> `Provider`
13. Permission/side-effect exit -> `Gateway`
