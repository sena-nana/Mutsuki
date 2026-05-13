"""Plugin 元类 + ``@command`` 装饰器。

面向用户的 API 故意保持极小：

* 子类化 :class:`Plugin`，声明 ``ClassVar`` ``id`` / ``version`` /
  ``capabilities``，以及一个嵌套的 ``Config(msgspec.Struct)``。
* 用 :func:`command` 装饰命令方法。

其他全部由 :class:`PluginMeta` 在 class 定义阶段完成（manifest 构造、命令
收集、schema 合成、docstring 解析、注册到 :data:`PluginRegistry`）。

为什么用真元类（而不是 ``__init_subclass__``）：

* manifest 校验在 ``class`` 语句求值时立即跑 —— 错误指向定义点本身，
  发生在任何模块级副作用（比如 adapter 安装）之前。
* manifest 字段以 ``ClassVar`` 形式声明（pyright 友好），不是 class 语句
  的关键字参数；元类直接从 ``cls.__dict__`` 读取。
* 与 ``msgspec.Struct`` 的元类无冲突：:class:`Plugin` 是普通 ABC，不是
  Struct。嵌套的 ``Config`` 是 Struct，独立用自己的元类。
"""

from __future__ import annotations

from abc import ABC, ABCMeta
from collections.abc import Awaitable, Callable
from dataclasses import dataclass
import inspect
from typing import (
    TYPE_CHECKING,
    Any,
    ClassVar,
    Generic,
    TypeVar,
    get_type_hints,
)

import docstring_parser
import msgspec

from nanobot.contracts.capability import Capability, CapabilityName
from nanobot.contracts.error import Error, Errs
from nanobot.contracts.permission import PermissionName, PermissionRule
from nanobot.contracts.permission_builtin import Perms
from nanobot.contracts.plugin import (
    Arg,
    CommandSpec,
    ContractDep,
    PluginDep,
    PluginManifest,
    ServiceDep,
)
from nanobot.core.dependency import (
    Dependent,
    UnresolvedParameterError,
)
from nanobot.core.registry import PluginRegistry

if TYPE_CHECKING:
    from nanobot.core.agent import Agent
    from nanobot.core.bus import Bus
    from nanobot.core.container import ServiceContainer
    from nanobot.core.scope import PluginScope


C = TypeVar("C", bound=msgspec.Struct)


class PluginDefinitionError(Exception):
    """:class:`PluginMeta` 在 Plugin 子类定义不合法时抛出。"""

    def __init__(self, message: str, *, error: Error) -> None:
        super().__init__(message)
        self.error = error


# ---------------------------------------------------------------------------
# @command 装饰器（声明侧的标记）
# ---------------------------------------------------------------------------


@dataclass(slots=True)
class _CommandMarker:
    """由 ``@command`` 附加在方法上，供元类后续收集。

    ``dependent`` 与 ``spec`` 在 :class:`PluginMeta` 解析阶段一次性填入，
    scheduler 命令分发时直接复用，避免 per-tick 重复 inspect / 线性查表。
    """

    func: Callable[..., Awaitable[Any]]
    perms: PermissionRule
    requires_capabilities: tuple[CapabilityName, ...]
    is_tool: bool
    explicit_name: str | None
    explicit_desc: str | None
    dependent: "Dependent[Any] | None" = None
    spec: CommandSpec | None = None


def command(
    *,
    name: str | None = None,
    desc: str | None = None,
    perms: PermissionRule | PermissionName | None = None,
    requires_capabilities: tuple[CapabilityName, ...] = (),
    is_tool: bool = True,
) -> Callable[[Callable[..., Awaitable[Any]]], Callable[..., Awaitable[Any]]]:
    """把一个 async 方法标记为 Plugin 命令（默认同时是 LLM tool）。

    装饰器只在函数对象上挂元数据；真正的 :class:`CommandSpec` 由
    :class:`PluginMeta` 在所属类体求值完毕后构建 —— 那时候方法 docstring、
    签名、所属类都已就位。
    """

    rule: PermissionRule
    if perms is None:
        rule = Perms.PUBLIC.to_rule()
    elif isinstance(perms, PermissionName):
        rule = perms.to_rule()
    else:
        rule = perms

    def decorator(fn: Callable[..., Awaitable[Any]]) -> Callable[..., Awaitable[Any]]:
        if not inspect.iscoroutinefunction(fn):
            raise TypeError(
                f"@command 修饰的 {fn.__qualname__} 必须是 `async def`"
            )
        fn.__command_marker__ = _CommandMarker(  # type: ignore[attr-defined]
            func=fn,
            perms=rule,
            requires_capabilities=requires_capabilities,
            is_tool=is_tool,
            explicit_name=name,
            explicit_desc=desc,
        )
        return fn

    return decorator


# ---------------------------------------------------------------------------
# JSON-Schema 合成（轻量，不引外部依赖）
# ---------------------------------------------------------------------------


_PRIMITIVE_TO_JSON: dict[type, str] = {
    str: "string",
    int: "integer",
    float: "number",
    bool: "boolean",
}


def _json_type_for(annotation: Any) -> dict[str, Any]:
    """对 Python 注解给出尽可能贴近的 JSON Schema 类型。识别不出退化为 ``string``。"""
    from typing import get_args, get_origin

    if get_origin(annotation) is not None:
        # 尝试 Annotated[T, ...]
        from typing import Annotated

        if get_origin(annotation) is Annotated:
            return _json_type_for(get_args(annotation)[0])
    if isinstance(annotation, type):
        json_type = _PRIMITIVE_TO_JSON.get(annotation)
        if json_type is not None:
            return {"type": json_type}
    return {"type": "string"}


def _build_command_spec(
    *,
    plugin_id: str,
    cls_qualname: str,
    marker: _CommandMarker,
    perms_rule_id: str,
) -> CommandSpec:
    fn = marker.func
    fn_name = marker.explicit_name or fn.__name__

    raw_doc = inspect.getdoc(fn) or ""
    parsed = docstring_parser.parse(raw_doc) if raw_doc else None
    description = (
        marker.explicit_desc
        or (parsed.short_description if parsed and parsed.short_description else "")
        or fn_name
    )
    param_descs: dict[str, str] = {}
    if parsed is not None:
        for p in parsed.params:
            if p.description:
                param_descs[p.arg_name] = p.description.strip()

    sig = inspect.signature(fn)
    try:
        hints = get_type_hints(fn, include_extras=True)
    except Exception:
        hints = {}

    properties: dict[str, dict[str, Any]] = {}
    required: list[str] = []
    return_schema: dict[str, Any] = {}

    for pname, sig_param in sig.parameters.items():
        if pname == "self":
            continue
        ann = hints.get(pname, sig_param.annotation)
        if ann is inspect.Parameter.empty:
            continue
        # 跳过 CtxParam 风格与 Inject() 默认值 —— 它们由框架注入。
        from typing import Annotated, get_args, get_origin

        from nanobot.contracts.plugin import Inject as _Inject
        from nanobot.core.context import AgentContext as _Ctx

        bare = ann
        if get_origin(ann) is Annotated:
            bare = get_args(ann)[0]
        if isinstance(bare, type) and issubclass(bare, _Ctx):
            continue
        if isinstance(sig_param.default, _Inject):
            continue

        prop: dict[str, Any] = _json_type_for(ann)
        # 来自 Annotated[..., Arg(...)] 的约束。
        if get_origin(ann) is Annotated:
            for meta in get_args(ann)[1:]:
                if isinstance(meta, Arg):
                    if meta.ge is not None:
                        prop["minimum"] = meta.ge
                    if meta.le is not None:
                        prop["maximum"] = meta.le
                    if meta.gt is not None:
                        prop["exclusiveMinimum"] = meta.gt
                    if meta.lt is not None:
                        prop["exclusiveMaximum"] = meta.lt
                    if meta.min_length is not None:
                        prop["minLength"] = meta.min_length
                    if meta.max_length is not None:
                        prop["maxLength"] = meta.max_length
                    if meta.regex is not None:
                        prop["pattern"] = meta.regex
                    if meta.choices is not None:
                        prop["enum"] = list(meta.choices)
                    if meta.desc and pname not in param_descs:
                        param_descs[pname] = meta.desc

        if pname in param_descs:
            prop["description"] = param_descs[pname]
        properties[pname] = prop

        if sig_param.default is inspect.Parameter.empty:
            required.append(pname)

    parameters_schema: dict[str, Any] = {
        "type": "object",
        "properties": properties,
        "required": required,
    }

    ret_ann = hints.get("return")
    if ret_ann is not None:
        return_schema = _json_type_for(ret_ann)

    return CommandSpec(
        name=fn_name,
        description=description,
        plugin_id=plugin_id,
        func_qualname=f"{cls_qualname}.{fn.__name__}",
        parameters_schema=parameters_schema,
        return_schema=return_schema,
        perms_rule_id=perms_rule_id,
        requires_capabilities=tuple(marker.requires_capabilities),
        is_tool=marker.is_tool,
    )


# ---------------------------------------------------------------------------
# PluginMeta —— 元类本体
# ---------------------------------------------------------------------------


_REQUIRED_CLASSVARS = ("id", "version", "capabilities")


class PluginMeta(ABCMeta):
    """在 Plugin 子类定义时校验、收集并注册。"""

    def __new__(
        mcs,
        name: str,
        bases: tuple[type, ...],
        namespace: dict[str, Any],
        **kwargs: Any,
    ) -> "PluginMeta":
        cls = super().__new__(mcs, name, bases, namespace, **kwargs)
        # 跳过抽象基类自身。
        if not bases or all(b is object for b in bases):
            return cls
        if name == "Plugin":
            return cls

        # 1) ClassVar manifest 字段。
        for field in _REQUIRED_CLASSVARS:
            if field not in cls.__dict__:
                err = Error(
                    code=Errs.PLUGIN_DEFINITION_ERROR,
                    source=cls.__module__,
                    route="plugin.define",
                    evidence={"missing": field, "plugin_class": name},
                )
                raise PluginDefinitionError(
                    f"Plugin {name!r} 缺少必需 ClassVar `{field}`",
                    error=err,
                )

        plugin_id = cls.__dict__["id"]
        plugin_version = cls.__dict__["version"]
        capabilities: tuple[Capability, ...] = tuple(
            cls.__dict__.get("capabilities") or ()
        )

        # 2) 嵌套 Config struct。
        config_cls = cls.__dict__.get("Config")
        if config_cls is None or not (
            isinstance(config_cls, type) and issubclass(config_cls, msgspec.Struct)
        ):
            err = Error(
                code=Errs.PLUGIN_DEFINITION_ERROR,
                source=cls.__module__,
                route="plugin.define",
                evidence={"plugin_class": name},
            )
            raise PluginDefinitionError(
                f"Plugin {name!r} 必须定义嵌套 `Config(msgspec.Struct)`",
                error=err,
            )

        # 3) 收集 @command 标记的方法，构造 CommandSpec。
        commands: list[CommandSpec] = []
        markers: dict[str, _CommandMarker] = {}
        for attr, value in namespace.items():
            marker = getattr(value, "__command_marker__", None)
            if isinstance(marker, _CommandMarker):
                markers[attr] = marker
                # 立刻校验签名可解析（提前反馈），并缓存 Dependent 供 scheduler 复用。
                try:
                    marker.dependent = Dependent.parse(value)
                except UnresolvedParameterError as e:
                    err = Error(
                        code=Errs.PLUGIN_DEFINITION_ERROR,
                        source=cls.__module__,
                        route="plugin.define",
                        evidence={
                            "plugin_class": name,
                            "method": attr,
                            "reason": str(e),
                        },
                    )
                    raise PluginDefinitionError(
                        f"Plugin {name!r} 命令 {attr!r} 签名非法: {e}",
                        error=err,
                    ) from e
                spec = _build_command_spec(
                    plugin_id=plugin_id,
                    cls_qualname=cls.__qualname__,
                    marker=marker,
                    perms_rule_id=f"{plugin_id}.{attr}",
                )
                marker.spec = spec
                commands.append(spec)

        cls.__commands__ = tuple(commands)  # type: ignore[attr-defined]
        cls.__command_markers__ = markers  # type: ignore[attr-defined]

        # 4) 构造静态 manifest。
        config_schema_id = getattr(config_cls, "schema_id", "") or (
            f"{plugin_id}.config"
        )
        manifest = PluginManifest(
            id=plugin_id,
            version=plugin_version,
            contracts=tuple(cls.__dict__.get("contracts", ()) or ()),
            capabilities=capabilities,
            provides_services=tuple(cls.__dict__.get("provides_services", ()) or ()),
            requires_services=tuple(cls.__dict__.get("requires_services", ()) or ()),
            requires_plugins=tuple(cls.__dict__.get("requires_plugins", ()) or ()),
            config_schema_id=config_schema_id,
            commands=tuple(commands),
        )
        cls.__manifest__ = manifest  # type: ignore[attr-defined]

        # 5) 登记到全局 PluginRegistry。
        PluginRegistry.register(plugin_id, cls)  # type: ignore[arg-type]

        # 6) 提供精确 __repr__（含来源 file:line，方便调试）。
        try:
            src_file = inspect.getsourcefile(cls) or "<unknown>"
            src_line = inspect.getsourcelines(cls)[1]
            cls.__source_location__ = f"{src_file}:{src_line}"  # type: ignore[attr-defined]
        except (OSError, TypeError):
            cls.__source_location__ = "<unknown>"  # type: ignore[attr-defined]

        return cls

    def __repr__(cls) -> str:
        loc = getattr(cls, "__source_location__", "<unknown>")
        pid = getattr(cls, "id", "<no-id>")
        return f"<Plugin {pid!r} at {loc}>"


# ---------------------------------------------------------------------------
# Plugin（虚拟基类）
# ---------------------------------------------------------------------------


class Plugin(ABC, Generic[C], metaclass=PluginMeta):
    """所有 NanoBot 插件的基类。

    子类必须声明这些 ``ClassVar``：

    * ``id: ClassVar[str]`` —— kebab-case，全局唯一。
    * ``version: ClassVar[str]`` —— SemVer。
    * ``capabilities: ClassVar[list[Capability]]``。

    子类必须定义嵌套 ``class Config(msgspec.Struct)``。

    框架在 :meth:`__init__`（由 loader 调用）注入 ``self.agent``、
    ``self.config``、``self.scope``、``self.services``、``self.bus``。
    被 :func:`command` 装饰的方法同时成为人类命令路由和 LLM tool。
    """

    id: ClassVar[str]
    version: ClassVar[str]
    capabilities: ClassVar[list[Capability]]
    contracts: ClassVar[list[ContractDep]] = []
    requires_plugins: ClassVar[list[PluginDep]] = []
    requires_services: ClassVar[list[ServiceDep]] = []
    provides_services: ClassVar[list[ServiceDep]] = []
    Config: ClassVar[type[msgspec.Struct]]

    __manifest__: ClassVar[PluginManifest]
    __commands__: ClassVar[tuple[CommandSpec, ...]]
    __command_markers__: ClassVar[dict[str, _CommandMarker]]
    __source_location__: ClassVar[str]

    def __init__(
        self,
        *,
        agent: "Agent",
        config: C,
        scope: "PluginScope",
        services: "ServiceContainer",
        bus: "Bus",
    ) -> None:
        self.agent = agent
        self.config: C = config
        self.scope = scope
        self.services = services
        self.bus = bus

    async def on_load(self) -> None:
        """重写以注册订阅、定时器、服务。默认空实现。"""

    async def on_unload(self) -> None:
        """重写以做显式清理。默认空实现（scope 仍会被关闭）。"""


__all__ = ["Plugin", "PluginDefinitionError", "PluginMeta", "command"]
