# API · `mutsukicore.core`

Agent 运行时核心：Agent、AgentRegistry、Dispatcher、scope、PluginMeta、bus、容器、loader、handle、ResourceHost、saga、lifespan、capability 守卫。

## 模块地图

| 模块 | 公开符号 |
|---|---|
| [`agent`](#agent) | `Agent` |
| [`agent_registry`](#agent_registry) | `AgentRegistry` |
| [`context`](#context) | `AgentContext` / `TraceContext` |
| [`plugin`](#plugin) | `Plugin` / `PluginMeta` / `PluginDefinitionError` / `command` |
| [`dependency`](#dependency) | `Dependent` / `Param` / `CtxParam` / `ArgParam` / `ServiceParam` / `RefParam` / `ParameterInfo` / `UnresolvedParameterError` |
| [`scope`](#scope) | `PluginScope` / `TransactionScope` / `ResourceKind` / `HandleLeakError` |
| [`bus`](#bus) | `Bus` / `EventHandler` |
| [`container`](#container) | `ServiceContainer` / `ServiceNotFoundError` |
| [`handle`](#handle) | `RefCountedHandle` / `HandleImpl` / `HandleNotAttachedError` / `HandleUseAfterReleaseError` / `make_stub_handle` |
| [`resource_host`](#resource_host) | `ResourceHost` / `ResourceLease` / `CapabilityExhaustedError` |
| [`registry`](#registry) | `PluginRegistry` / `HandleRegistry` / `RegistryConflictError` |
| [`loader`](#loader) | `PluginLoader` / `PluginCycleError` / `PluginDependencyMissingError` / `PluginLoadFailedError` / `PluginConfigInvalidError` / `PluginNotFoundError` |
| [`lifespan`](#lifespan) | `Lifespan` / `Hook` |
| [`saga`](#saga) | `Saga` / `SagaCompensationError` |
| [`capability_guard`](#capability_guard) | `check_capabilities` / `CapabilityNotDeclaredError` |

---

## agent

[agent.py](../../mutsukicore/core/agent.py) · 详见 [Agent 与生命周期](../04-guide/agent-and-lifecycle.md)。

```python
@dataclass
class Agent:
    agent_id: AgentId
    clock: Clock
    id_gen: IdGen
    rng: RNG
    owner: str | None = None
    priority: int = 0
    accepts: tuple[ScopeRule, ...] = ()
    services: ServiceContainer = ...
    bus: Bus = ...
    lifespan: Lifespan = ...
    inbox: asyncio.Queue[object] = ...
    outbox: asyncio.Queue[Message] = ...
    phase: LifecyclePhase = AWAKE
    plugins: list[_LoadedPlugin] = ...
    _agent_scope: PluginScope | None = None

    def __post_init__(self) -> None
    @property
    def dispatch(self) -> Dispatcher
    def make_context(self, message: Message | None = None) -> AgentContext
    def attach_plugin(self, plugin: Plugin, scope: PluginScope) -> None
    async def close_agent_scope(self) -> None
```

`Agent.__post_init__` 会自动登记到 [`AgentRegistry`](#agent_registry)。v0.2 起命令路由统一走 `Dispatcher`；`CommandTarget` / `_command_index` / `find_command` 已删除。

## agent_registry

[agent_registry.py](../../mutsukicore/core/agent_registry.py)

```python
class _AgentRegistry:
    def register(self, agent: Agent) -> None
    def unregister(self, agent: Agent | str) -> None
    def get(self, agent_id: str) -> Agent | None
    def all(self) -> tuple[Agent, ...]
    def clear(self) -> None
    def install_election_policy(
        self, policy: AgentElectionPolicy, *, owner: str
    ) -> Callable[[], None]
    def rank_accepting(self, envelope: Envelope) -> tuple[Agent, ...]
    def select_accepting(self, envelope: Envelope) -> Agent | None
    def iter_accepting(self, envelope: Envelope) -> Iterator[Agent]

AgentRegistry: _AgentRegistry
```

`AgentRegistry` 用弱引用保存进程内 Agent。默认 election policy 按 `priority` 降序、`agent_id` 升序排序；`install_election_policy(...)` 可临时替换排序策略，并返回幂等 disposer。`rank_accepting(envelope)` 返回当前策略排序后的候选，`select_accepting(envelope)` 返回单个 winner。`Dispatcher.publish()` 用 `iter_accepting(envelope)` 广播给所有 `phase == AWAKE` 且 `accepts` 命中的 Agent。

## context

[context.py](../../mutsukicore/core/context.py) · 详见 [AgentContext](../04-guide/agent-context.md)。

```python
@dataclass(slots=True)
class TraceContext:
    trace_id: TraceId
    span_id: SpanId
    parent_span_id: SpanId | None = None

@dataclass(slots=True)
class AgentContext:
    agent_id: AgentId
    agent_owner: str | None
    clock: Clock
    id_gen: IdGen
    rng: RNG
    services: ServiceContainer
    scope: PluginScope
    bus: Bus
    dispatch: Dispatcher
    trace_ctx: TraceContext
    message: Message | None = None
    extras: dict[str, object] = ...
```

## plugin

[plugin.py](../../mutsukicore/core/plugin.py) · 详见 [插件定义](../04-guide/plugin-definition.md)。

```python
class Plugin(ABC, Generic[C], metaclass=PluginMeta):
    id: ClassVar[str]
    version: ClassVar[str]
    capabilities: ClassVar[list[Capability]]
    contracts: ClassVar[list[ContractDep]] = []
    requires_plugins: ClassVar[list[PluginDep]] = []
    requires_services: ClassVar[list[ServiceDep]] = []
    provides_services: ClassVar[list[ServiceDep]] = []
    consumes: ClassVar[tuple[ScopeRule, ...]] = ()
    provides_operations: ClassVar[tuple[OperationDescriptor, ...]] = ()
    provides_sources: ClassVar[tuple[SourceDescriptor, ...]] = ()
    requires_operations: ClassVar[tuple[OperationDep, ...]] = ()
    requires_sources: ClassVar[tuple[SourceDep, ...]] = ()
    Config: ClassVar[type[msgspec.Struct]]

    __manifest__: ClassVar[PluginManifest]
    __commands__: ClassVar[tuple[CommandSpec, ...]]
    __command_markers__: ClassVar[dict[str, _CommandMarker]]
    __source_location__: ClassVar[str]

    def __init__(self, *, agent, config, scope, services, bus): ...
    async def on_load(self) -> None: ...
    async def on_unload(self) -> None: ...
    async def on_envelope(self, envelope: Envelope) -> None: ...

class PluginMeta(ABCMeta): ...

def command(*, name=None, desc=None, perms=None, requires_capabilities=(), is_tool=True): ...

class PluginDefinitionError(Exception):
    error: Error
```

## dependency

[dependency.py](../../mutsukicore/core/dependency.py) · 详见 [依赖注入](../04-guide/dependency-injection.md)。

```python
@dataclass(frozen=True, slots=True)
class ParameterInfo:
    name: str
    annotation: Any
    default: Any
    has_default: bool
    annotated_metadata: tuple[Any, ...]

class Param(ABC):
    @classmethod
    @abstractmethod
    def claim(cls, info: ParameterInfo) -> Param | None
    @abstractmethod
    async def solve(self, ctx, **extras) -> Any

class CtxParam(Param): ...
class ArgParam(Param): ...
class ServiceParam(Param): ...
class RefParam(Param): ...

@dataclass(frozen=True, slots=True)
class Dependent(Generic[R]):
    call: Callable[..., Awaitable[R]]
    params: tuple[Param, ...] = ()

    @classmethod
    def parse(cls, call, *, allow_types=_DEFAULT_PARAMS, skip_self=True) -> Dependent[R]
    async def solve(self, ctx, bound_self=None, **extras) -> R

class UnresolvedParameterError(TypeError): ...
```

## scope

[scope.py](../../mutsukicore/core/scope.py) · 详见 [PluginScope](../04-guide/plugin-scope.md) · [TransactionScope 与 Saga](../05-advanced/transaction-scope-saga.md)。

```python
CleanupFn = Callable[[], None] | Callable[[], Awaitable[None]]

class ResourceKind(StrEnum):
    SUBSCRIPTION, TIMER, SERVICE_REGISTRATION, CONTEXT_ATTACHMENT, CONFIG_WATCHER
    DISPATCH_REGISTRATION, GENERIC_DISPOSE

class HandleLeakError(Exception):
    leaked: list[RefId]
    error: Error

class PluginScope:
    owner: str
    closed: bool
    def add_subscription(self, fn: CleanupFn) -> None
    def add_timer(self, fn: CleanupFn) -> None
    def add_service_registration(self, fn: CleanupFn) -> None
    def add_context_attachment(self, fn: CleanupFn) -> None
    def add_config_watcher(self, fn: CleanupFn) -> None
    def add_dispatch_registration(self, fn: CleanupFn) -> None
    def add_dispose(self, fn: CleanupFn) -> None
    def attach_handle(self, handle: Handle[object]) -> None
    async def close(self) -> None

class TransactionScope(PluginScope):
    def register_compensation(self, fn: CleanupFn) -> None
    async def commit(self) -> None
    async def rollback(self) -> None
```

## bus

[bus.py](../../mutsukicore/core/bus.py) · 详见 [事件总线](../04-guide/event-bus.md)。

```python
EventHandler = Callable[[object], Awaitable[None]]

@dataclass(slots=True)
class Bus:
    def subscribe(
        self, event_type: str, handler: EventHandler, *, direct: bool = False
    ) -> Callable[[], None]
    async def publish(self, event_type: str, payload: object) -> None
```

## container

[container.py](../../mutsukicore/core/container.py) · 详见 [服务容器](../04-guide/service-container.md)。

```python
class ServiceContainer:
    def register(self, contract: type, instance: Any, *, name: str | None = None) -> None
    def unregister(self, contract: type, instance: Any) -> None
    def resolve(self, contract: type, *, name: str | None = None) -> Any
    def has(self, contract: type, *, name: str | None = None) -> bool

class ServiceNotFoundError(KeyError): ...
```

## handle

[handle.py](../../mutsukicore/core/handle.py) · 详见 [Handle 与 RefPayload](../04-guide/handle-and-refpayload.md)。

```python
class HandleImpl(Handle[T], Generic[T]):
    handle_kind: str = "generic"
    def __init_subclass__(cls, **kwargs) -> None  # 自动注册到 HandleRegistry

class RefCountedHandle(HandleImpl[T], Generic[T]):
    def __init__(self, target: T, descriptor: RefDescriptor, finalizer=None)
    @property
    def ref_id(self) -> RefId
    @property
    def descriptor(self) -> RefDescriptor
    def attach_to(self, scope) -> None
    def acquire(self) -> T
    def release(self) -> None
    @contextmanager
    def borrow(self) -> Generator[T]
    def is_alive(self) -> bool

def make_stub_handle(
    ref_id: RefId, *,
    kind: str = "test.stub",
    schema_id_target: str = "test.stub/v1",
    schema_version_target: str = "1.0.0",
    target: object = None,
    attributes: dict | None = None,
) -> RefCountedHandle[object]

class HandleNotAttachedError(Exception):
    ref_id: RefId
    error: Error

class HandleUseAfterReleaseError(Exception):
    ref_id: RefId
    error: Error
```

## resource_host

[resource_host.py](../../mutsukicore/core/resource_host.py) · 详见 [ResourceHost](../04-guide/resource-host.md)。

```python
class ResourceHost:
    def __init__(
        self, *, owner: str = "resource-host",
        policy_config: ResourceHostPolicyConfig | Mapping[str, object] | None = None,
        eviction_policy: ResourceEvictionPolicy | None = None,
        keepalive_policy: ResourceKeepalivePolicy | None = None,
    ) -> None
    def create_handle(
        self, ref_id: RefId, *, target: T, kind: str,
        schema_id_target: str, schema_version_target: str,
        attributes: dict | None = None, finalizer=None,
    ) -> RefCountedHandle[T]
    def get_handle(self, ref_id: RefId, *, kind: str | None = None) -> RefCountedHandle[Any]
    async def get_handle_for(
        self, ctx: AgentContext, ref_id: RefId, *, kind: str | None = None
    ) -> RefCountedHandle[Any]
    def declare_capacity(self, capability: CapabilityName, *, total: int) -> None
    def acquire(self, capability: CapabilityName, *, amount: int = 1, owner: str) -> ResourceLease
    async def acquire_for(
        self, ctx: AgentContext, capability: CapabilityName, *, amount: int = 1, owner: str
    ) -> ResourceLease
    async def release_for(self, ctx: AgentContext, lease: ResourceLease) -> None
    def evict(self, policy: ResourceEvictionPolicy | None = None) -> tuple[RefId, ...]
    async def keepalive(
        self, policy: ResourceKeepalivePolicy | None = None
    ) -> tuple[RefId, ...]
    async def close(self) -> None

class ResourceLease:
    capability: CapabilityName
    amount: int
    owner: str
    alive: bool
    def release(self) -> None

class CapabilityExhaustedError(Exception):
    error: Error

class ResourceHandleNotFoundError(Exception):
    error: Error
```

## registry

[registry.py](../../mutsukicore/core/registry.py)

```python
class _NamedRegistry(Generic[_T]):
    def register(self, key: str, value: _T) -> None
    def unregister(self, key: str) -> None
    def get(self, key: str) -> _T | None
    def require(self, key: str) -> _T
    def __iter__ / __contains__ / __len__ / clear

PluginRegistry: _NamedRegistry[type[Plugin]]
HandleRegistry: _NamedRegistry[type[HandleImpl]]

class RegistryConflictError(Exception): ...
```

## loader

[loader.py](../../mutsukicore/core/loader.py) · 详见 [插件 DAG 加载](../05-advanced/plugin-loader-dag.md)。

```python
class PluginLoader:
    def __init__(self, *, entry_point_group="mutsukicore.plugins", allow=None) -> None
    def discover(self) -> list[type[Plugin]]
    async def load_into(
        self, agent: Agent,
        plugin_classes: Iterable[type[Plugin]],
        configs: Mapping[str, object] | None = None,
    ) -> None
    async def unload_from(self, agent: Agent) -> None

class PluginCycleError(Exception):
    cycle: list[str]
    error: Error

class PluginDependencyMissingError(Exception):
    missing: list[tuple[str, str]]
    error: Error

class PluginLoadFailedError(Exception):
    plugin_id: str
    error: Error

class PluginConfigInvalidError(Exception):
    plugin_id: str
    error: Error

class PluginNotFoundError(KeyError): ...
```

## lifespan

[lifespan.py](../../mutsukicore/core/lifespan.py)

```python
Hook = Callable[[AgentContext], Awaitable[None]]

@dataclass(slots=True)
class Lifespan:
    on_awake: list[Hook] = ...
    on_sleep: list[Hook] = ...
    on_stop: list[Hook] = ...
    async def fire(self, phase: str, ctx: AgentContext) -> None
```

`fire("awake")` 按声明顺序；`fire("sleep"|"stop")` 反序（LIFO）。

## saga

[saga.py](../../mutsukicore/core/saga.py) · 详见 [TransactionScope 与 Saga](../05-advanced/transaction-scope-saga.md)。

```python
ForwardFn = Callable[[], Awaitable[Any]]
CompensateFn = Callable[[], Awaitable[None]]

@dataclass(slots=True)
class Saga:
    owner: str = "core.saga"
    def add_step(self, forward: ForwardFn, compensate: CompensateFn, *, name=None) -> None
    async def run(self) -> list[Any]

class SagaCompensationError(Exception):
    original: BaseException
    comp_errors: list[BaseException]
    error: Error
```

## capability_guard

[capability_guard.py](../../mutsukicore/core/capability_guard.py) · 详见 [Capability](../04-guide/capability.md)。

```python
def check_capabilities(
    *,
    plugin_id: str,
    declared: tuple[CapabilityName, ...],
    required: tuple[CapabilityName, ...],
    route: str,
) -> None  # 不通过抛 CapabilityNotDeclaredError

class CapabilityNotDeclaredError(Exception):
    missing: tuple[CapabilityName, ...]
    error: Error
```
