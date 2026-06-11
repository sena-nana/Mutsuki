# API · `mutsukicore.contracts`

类型定义包；除三个内置门面（Caps / Perms / Errs）的注册副作用外，无运行时副作用。

来源：[mutsukicore/contracts/__init__.py](../../mutsukicore/contracts/__init__.py)。

## 模块地图

| 模块 | 内容 |
|---|---|
| [`base`](#base) | `Contract` / `SchemaRegistry` / `SchemaConflictError` |
| [`ids`](#ids) | `AgentId` / `MessageId` / `RefId` / `TraceId` / `SpanId`（NewType） |
| [`lifecycle`](#lifecycle) | `LifecyclePhase` 枚举 |
| [`message`](#message) | `Message` / `ContentPart` / `ContentKind` / `ChannelRef` |
| [`event`](#event) | `Event` / `TraceSpan` / `SpanStatus` |
| [`capability`](#capability) | `Capability` / `CapabilityName` / `UnknownCapabilityError` / `CapabilityConflictError` |
| [`capability_builtin`](#capability_builtin) | `Caps` 门面 |
| [`permission`](#permission) | `PermissionRule` / `PermissionName` / `CheckerFn` / 错误类型 |
| [`permission_builtin`](#permission_builtin) | `Perms` 门面 |
| [`error`](#error) | `Error` / `ErrorCode` / `RecoveryAction` / `Errs` 门面 |
| [`refpayload`](#refpayload) | `Handle` / `RefPayload` / `RefDescriptor` / `BackpressureChannel` / `Replayability` |
| [`plugin`](#plugin) | `PluginManifest` / `CommandSpec` / `Arg` / `Inject` / `RefArg` / 三种 Dep |
| [`service`](#service) | `Service` Protocol / `ServiceMode` |
| [`schema`](#schema) | `register_schema_compatibility` / `is_compatible` |
| [`decision`](#decision) | `Decision` |

---

## base

[base.py](../../mutsukicore/contracts/base.py)

- `Contract` —— `msgspec.Struct` 子类，要求 ClassVar `schema_id` / `schema_version`；`__init_subclass__` 自动注册到 `SchemaRegistry`
- `SchemaRegistry` —— 进程内单例（按 `schema_id` 索引契约类）
- `SchemaConflictError`

## ids

[ids.py](../../mutsukicore/contracts/ids.py)

`AgentId` / `TraceId` / `SpanId` / `RefId` / `MessageId` —— 全是 `NewType("...", str)`。pyright 会标记跨 ID 误赋值。

## lifecycle

[lifecycle.py](../../mutsukicore/contracts/lifecycle.py)

`LifecyclePhase`：`SPAWN` / `AWAKE` / `SLEEP` / `STOP`。

## message

[message.py](../../mutsukicore/contracts/message.py)

```python
class ContentKind(StrEnum):
    TEXT, IMAGE_REF, AUDIO_REF, FILE_REF, LATENT_REF, TOOL_SCHEMA_REF

class ChannelRef(Contract):
    source_id: str
    channel_id: str
    user_id: str | None = None

class ContentPart(Contract):
    kind: ContentKind
    text: str | None = None
    ref: RefDescriptor | None = None
    metadata: dict[str, str] = {}

class Message(Contract):
    id: MessageId
    timestamp: float
    source: ChannelRef
    parts: tuple[ContentPart, ...]
    capabilities_required: tuple[CapabilityName, ...] = ()

    @property
    def text(self) -> str: ...   # 拼接所有 TEXT 片段
```

## event

[event.py](../../mutsukicore/contracts/event.py)

```python
class SpanStatus(StrEnum):
    OK, ERROR

class TraceSpan(Contract):
    trace_id: TraceId
    span_id: SpanId
    parent_span_id: SpanId | None = None
    name: str = ""
    start: float = 0.0
    end: float | None = None
    attributes: dict[str, str|int|float|bool] = {}
    status: SpanStatus = SpanStatus.OK

class Event(Contract):
    id: str
    timestamp: float
    type: str
    source_plugin: str
    payload: msgspec.Raw
    trace_id: TraceId
    span_id: SpanId
    parent_span_id: SpanId | None = None
```

详见 [Trace 与 Span](../04-guide/trace-and-span.md)。

## capability

[capability.py](../../mutsukicore/contracts/capability.py)

```python
class CapabilityName(RegisteredString): ...

class Capability(Contract):
    name: CapabilityName
    quantity: dict[str, int|str] | None = None
    policy: dict[str, str] | None = None

class UnknownCapabilityError(Exception): ...
class CapabilityConflictError(Exception): ...
```

详见 [Capability](../04-guide/capability.md)。

## capability_builtin

[capability_builtin.py](../../mutsukicore/contracts/capability_builtin.py)

`Caps.READ_MESSAGE` / `SEND_MESSAGE` / `CALL_LLM` / `PERSIST` / `NETWORK_EGRESS` / `SPAWN_AGENT` / `HOLD_REF` / `BORROW_REF` / `PRODUCE_REF_STREAM`，由 `bootstrap_facade` 注册到 `mutsukicore.core` owner。

## permission

[permission.py](../../mutsukicore/contracts/permission.py)

```python
class PermissionRule:
    async def check(self, ctx: AgentContext) -> bool
    @classmethod
    def from_checker(fn: CheckerFn) -> PermissionRule
    @classmethod
    def always() -> PermissionRule
    @classmethod
    def never() -> PermissionRule
    def __and__(other) -> PermissionRule
    def __or__(other) -> PermissionRule

class PermissionName(RegisteredString):
    @classmethod
    def register(name: str, *, declared_by: str, checker: CheckerFn) -> PermissionName
    def to_rule() -> PermissionRule

CheckerFn = Callable[[AgentContext], Awaitable[bool]]

class UnknownPermissionError(Exception): ...
class PermissionConflictError(Exception): ...
```

详见 [Permission](../04-guide/permission.md)。

## permission_builtin

[permission_builtin.py](../../mutsukicore/contracts/permission_builtin.py)

`Perms.PUBLIC` / `Perms.AGENT_OWNER`。

## error

[error.py](../../mutsukicore/contracts/error.py)

```python
class ErrorCode(RegisteredString): ...

class RecoveryAction(StrEnum):
    RETRY, FALLBACK, ESCALATE, ABORT

class Error(Contract):
    code: ErrorCode
    source: str
    route: str
    lost_capability: CapabilityName | None = None
    recovery: RecoveryAction | None = None
    cause: "Error | None" = None
    evidence: dict[str, str|int|float|bool] = {}
    def chain(self) -> list[Self]
```

`Errs.*`：14 个内置错误码（详见 [error-model](../04-guide/error-model.md)）。

## refpayload

[refpayload.py](../../mutsukicore/contracts/refpayload.py)

```python
class Replayability(StrEnum):
    FULL, INPUT_SEED_ONLY, NONE

class RefDescriptor(Contract):
    ref_id: RefId
    kind: str
    schema_id_target: str
    schema_version_target: str
    attributes: dict[str, str|int|float|bool] = {}
    lineage: tuple[RefId, ...] = ()

class Handle(ABC, Generic[T]):
    def acquire(self) -> T
    def release(self) -> None
    def borrow(self) -> AbstractContextManager[T]
    def is_alive(self) -> bool
    def attach_to(self, scope) -> None
    @property
    def ref_id(self) -> RefId
    @property
    def descriptor(self) -> RefDescriptor

class RefPayload(Contract, Generic[T]):
    ref_id: RefId
    handle: Handle[Any]
    descriptor: RefDescriptor

class BackpressureChannel(ABC, Generic[T]):
    high_watermark: int
    low_watermark: int
    async def send(self, item: T) -> None
    async def recv(self) -> T
    @property
    def closed(self) -> bool
    def close(self) -> None
```

详见 [Handle 与 RefPayload](../04-guide/handle-and-refpayload.md)。

## plugin

[plugin.py](../../mutsukicore/contracts/plugin.py)

```python
@dataclass(frozen=True, slots=True)
class Arg:
    desc: str | None = None
    ge / le / gt / lt: int | float | None = None
    min_length / max_length: int | None = None
    regex: str | None = None
    choices: tuple[str, ...] | None = None

@dataclass(frozen=True, slots=True)
class Inject:
    name: str | None = None

@dataclass(frozen=True, slots=True)
class RefArg:
    kind: str
    source: RefArgSource = RefArgSource.PAYLOAD
    ref_id: str | None = None
    host_name: str | None = None

class PluginDep(Contract):
    plugin_id: str
    version_range: str = "*"

class ContractDep(Contract):
    package: str
    version_range: str = "*"

class ServiceDep(Contract):
    name: str
    contract_id: str
    mode: ServiceMode = ServiceMode.BY_REF

class CommandSpec(Contract):
    name: str
    description: str
    plugin_id: str
    func_qualname: str
    parameters_schema: dict[str, Any] = {}
    return_schema: dict[str, Any] = {}
    perms_rule_id: str | None = None
    requires_capabilities: tuple[CapabilityName, ...] = ()
    is_tool: bool = True

class PluginManifest(Contract):
    id: str
    version: str
    contracts: tuple[ContractDep, ...] = ()
    capabilities: tuple[Capability, ...] = ()
    provides_services: tuple[ServiceDep, ...] = ()
    requires_services: tuple[ServiceDep, ...] = ()
    requires_plugins: tuple[PluginDep, ...] = ()
    config_schema_id: str = ""
    commands: tuple[CommandSpec, ...] = ()
```

## service

[service.py](../../mutsukicore/contracts/service.py)

```python
class ServiceMode(StrEnum):
    BY_VALUE, BY_REF

@runtime_checkable
class Service(Protocol):
    service_id: str
```

## schema

[schema.py](../../mutsukicore/contracts/schema.py)

```python
CompatibilityFn = Callable[[str, str], bool]

def register_schema_compatibility(schema_id: str, fn: CompatibilityFn) -> None
def is_compatible(schema_id: str, producer: str, consumer: str) -> bool
```

默认策略：版本字符串完全相等才视为兼容。

## decision

[decision.py](../../mutsukicore/contracts/decision.py)

```python
class Decision(Contract):
    id: str
    source: str
    route: str
    payload: msgspec.Raw
    alternatives_considered: tuple[str, ...] = ()
```

记录"选了什么、考虑过什么备选"。
