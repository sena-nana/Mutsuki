"""类型化依赖注入 —— 借鉴 NoneBot 的 ``Dependent`` / ``Param``。

与 NoneBot 的差别：

* 不允许按名 fallback。参数必须被以下二者之一认领：
  (a) 已安装的 ``Param`` 能识别的类型注解；
  (b) sentinel 默认值（``Inject()`` / ``Arg()`` 等）。
  否则在 parse 阶段抛 :class:`UnresolvedParameterError`。
* 只针对一种调用形式 —— ``async def fn(...) -> R`` —— 而设计，并被
  ``@command`` 装饰器复用，从同一份签名生成「人类命令路由」与「LLM tool」
  两份 schema。
"""

from __future__ import annotations

from abc import ABC, abstractmethod
from collections.abc import Awaitable, Callable
from dataclasses import dataclass, field
import inspect
from typing import (
    TYPE_CHECKING,
    Annotated,
    Any,
    Generic,
    TypeVar,
    get_args,
    get_origin,
    get_type_hints,
)

from mutsuki.contracts.error import Error, Errs
from mutsuki.contracts.ids import RefId
from mutsuki.contracts.plugin import Arg, Inject, RefArg, RefArgSource
from mutsuki.contracts.refpayload import Handle, RefPayload

if TYPE_CHECKING:
    from mutsuki.core.context import AgentContext


R = TypeVar("R")


@dataclass(frozen=True, slots=True)
class ParameterInfo:
    """所有 Param 子类共用的解析后参数描述。"""

    name: str
    annotation: Any
    default: Any
    has_default: bool
    annotated_metadata: tuple[Any, ...]


class UnresolvedParameterError(TypeError):
    """没有任何已安装 ``Param`` 能认领某个函数参数时抛出。"""


class RefResolutionError(Exception):
    """RefArg 解析失败时的结构化错误载体。"""

    def __init__(self, error: Error) -> None:
        super().__init__(f"ref resolution failed: {error.code}")
        self.error = error


class Param(ABC):
    """一个位置参数的解析器抽象基类。

    子类实现 :meth:`claim`（本 Param 是否要解析这个参数？）与 :meth:`solve`
    （调用时产生实际值）。
    """

    @classmethod
    @abstractmethod
    def claim(cls, info: ParameterInfo) -> "Param | None":
        """返回一个能解析 ``info`` 的实例；不能解析则返回 ``None``。"""

    @abstractmethod
    async def solve(self, ctx: "AgentContext", **extras: Any) -> Any: ...


@dataclass(frozen=True, slots=True)
class CtxParam(Param):
    """把当前 :class:`AgentContext` 注入名为 ``ctx`` 的参数。

    认领规则：参数注解是 ``AgentContext``（或子类）。
    """

    info: ParameterInfo

    @classmethod
    def claim(cls, info: ParameterInfo) -> "CtxParam | None":
        from mutsuki.core.context import AgentContext

        ann = _strip_annotated(info.annotation)
        try:
            if isinstance(ann, type) and issubclass(ann, AgentContext):
                return cls(info)
        except TypeError:
            pass
        return None

    async def solve(self, ctx: "AgentContext", **extras: Any) -> Any:
        return ctx


@dataclass(frozen=True, slots=True)
class ArgParam(Param):
    """注入由调用方提供的参数。

    认领规则：参数注解是 ``Annotated[T, Arg(...)]``，或者是任何普通类型
    （非 ``AgentContext``、``Service``、``Handle``）且默认值不是 sentinel。
    约束来自 ``Arg(...)``；描述由调用方从 docstring 提取。
    """

    info: ParameterInfo
    constraints: Arg

    @classmethod
    def claim(cls, info: ParameterInfo) -> "ArgParam | None":
        # 跳过 Inject sentinel 默认值 —— 它们交给 ServiceParam 处理。
        if isinstance(info.default, Inject):
            return None
        # 约束：取 metadata 里第一个 Arg()（如果有）。
        arg = next((m for m in info.annotated_metadata if isinstance(m, Arg)), None)
        # 跳过 RefArg —— 由 RefParam 处理。
        if any(isinstance(m, RefArg) for m in info.annotated_metadata):
            return None
        return cls(info, arg or Arg())

    async def solve(self, ctx: "AgentContext", **extras: Any) -> Any:
        if self.info.name not in extras:
            if self.info.has_default:
                return self.info.default
            raise KeyError(f"缺少参数: {self.info.name!r}")
        return extras[self.info.name]


@dataclass(frozen=True, slots=True)
class ServiceParam(Param):
    """从容器注入 service 或插件 Config。

    认领规则：默认值是 ``Inject()`` sentinel。
    """

    info: ParameterInfo
    inject: Inject

    @classmethod
    def claim(cls, info: ParameterInfo) -> "ServiceParam | None":
        if isinstance(info.default, Inject):
            return cls(info, info.default)
        return None

    async def solve(self, ctx: "AgentContext", **extras: Any) -> Any:
        ann = _strip_annotated(self.info.annotation)
        return ctx.services.resolve(ann, name=self.inject.name)


@dataclass(frozen=True, slots=True)
class RefParam(Param):
    """注入绑定到领域 RefArg(kind=...) 标记的 Handle。"""

    info: ParameterInfo
    ref: RefArg

    @classmethod
    def claim(cls, info: ParameterInfo) -> "RefParam | None":
        ref = next((m for m in info.annotated_metadata if isinstance(m, RefArg)), None)
        if ref is None:
            return None
        if not _is_handle_annotation(info.annotation):
            return None
        return cls(info, ref)

    async def solve(self, ctx: "AgentContext", **extras: Any) -> Any:
        if self.ref.source == RefArgSource.PAYLOAD:
            if self.info.name not in extras:
                raise KeyError(f"缺少 ref 参数: {self.info.name!r}")
            return _coerce_handle(
                extras[self.info.name],
                expected_kind=self.ref.kind,
                route=f"dependency.ref.{self.info.name}",
            )

        if self.ref.source == RefArgSource.RESOURCE_HOST:
            from mutsuki.core.container import ServiceNotFoundError
            from mutsuki.core.resource_host import ResourceHost

            ref_id_value = self.ref.ref_id or extras.get(self.info.name)
            if not isinstance(ref_id_value, str):
                err = Error(
                    code=Errs.REF_NOT_FOUND,
                    source="core.dependency",
                    route=f"dependency.ref.{self.info.name}",
                    evidence={
                        "parameter": self.info.name,
                        "expected_kind": self.ref.kind,
                        "reason": "missing_ref_id",
                    },
                )
                raise RefResolutionError(err)
            try:
                host = ctx.services.resolve(ResourceHost, name=self.ref.host_name)
            except ServiceNotFoundError as exc:
                err = Error(
                    code=Errs.SERVICE_NOT_FOUND,
                    source="core.dependency",
                    route=f"dependency.ref.{self.info.name}",
                    evidence={
                        "contract": "ResourceHost",
                        "name": self.ref.host_name or "",
                        "parameter": self.info.name,
                    },
                )
                raise RefResolutionError(err) from exc
            return await host.get_handle_for(
                ctx,
                RefId(ref_id_value),
                kind=self.ref.kind,
            )

        raise AssertionError(f"unknown RefArgSource: {self.ref.source!r}")


_DEFAULT_PARAMS: tuple[type[Param], ...] = (CtxParam, RefParam, ServiceParam, ArgParam)


@dataclass(frozen=True, slots=True)
class Dependent(Generic[R]):
    """已解析的签名：解析后的 Param 列表 + 原始可调用对象。"""

    call: Callable[..., Awaitable[R]]
    params: tuple[Param, ...] = field(default_factory=tuple)

    @classmethod
    def parse(
        cls,
        call: Callable[..., Awaitable[R]],
        *,
        allow_types: tuple[type[Param], ...] = _DEFAULT_PARAMS,
        skip_self: bool = True,
    ) -> "Dependent[R]":
        sig = inspect.signature(call)
        try:
            hints = get_type_hints(call, include_extras=True)
        except Exception:  # pragma: no cover —— 退化到原始注解
            hints = {p.name: p.annotation for p in sig.parameters.values()}

        resolved: list[Param] = []
        for name, sig_param in sig.parameters.items():
            if skip_self and name == "self":
                continue
            if sig_param.kind in (
                inspect.Parameter.VAR_POSITIONAL,
                inspect.Parameter.VAR_KEYWORD,
            ):
                raise UnresolvedParameterError(
                    f"{call.__qualname__}: 不支持可变参数"
                )

            ann = hints.get(name, sig_param.annotation)
            if ann is inspect.Parameter.empty:
                raise UnresolvedParameterError(
                    f"{call.__qualname__}: 参数 {name!r} 缺少类型注解"
                )

            metadata = _annotated_metadata(ann)
            info = ParameterInfo(
                name=name,
                annotation=ann,
                default=(
                    sig_param.default
                    if sig_param.default is not inspect.Parameter.empty
                    else None
                ),
                has_default=sig_param.default is not inspect.Parameter.empty,
                annotated_metadata=metadata,
            )

            for ParamCls in allow_types:
                claimed = ParamCls.claim(info)
                if claimed is not None:
                    resolved.append(claimed)
                    break
            else:
                raise UnresolvedParameterError(
                    f"{call.__qualname__}: 参数 {name!r} (类型 {ann!r}) "
                    f"未被任何已安装的 Param 认领"
                )

        return cls(call=call, params=tuple(resolved))

    async def solve(
        self,
        ctx: "AgentContext",
        bound_self: object | None = None,
        **extras: Any,
    ) -> R:
        kwargs: dict[str, Any] = {}
        for param in self.params:
            kwargs[param.info.name] = await param.solve(ctx, **extras)  # type: ignore[attr-defined]
        if bound_self is not None:
            return await self.call(bound_self, **kwargs)  # type: ignore[arg-type]
        return await self.call(**kwargs)


def _strip_annotated(annotation: Any) -> Any:
    """跳过 :data:`typing.Annotated` 包装，返回内层类型。"""
    if get_origin(annotation) is Annotated:
        return get_args(annotation)[0]
    return annotation


def _annotated_metadata(annotation: Any) -> tuple[Any, ...]:
    if get_origin(annotation) is Annotated:
        return get_args(annotation)[1:]
    return ()


def _is_handle_annotation(annotation: Any) -> bool:
    ann = _strip_annotated(annotation)
    origin = get_origin(ann)
    candidate = origin or ann
    try:
        return isinstance(candidate, type) and issubclass(candidate, Handle)
    except TypeError:
        return False


def _coerce_handle(value: Any, *, expected_kind: str, route: str) -> Handle[Any]:
    handle: Handle[Any]
    if isinstance(value, RefPayload):
        handle = value.handle
    elif isinstance(value, Handle):
        handle = value
    else:
        err = Error(
            code=Errs.REF_NOT_FOUND,
            source="core.dependency",
            route=route,
            evidence={
                "expected_kind": expected_kind,
                "actual_type": type(value).__qualname__,
            },
        )
        raise RefResolutionError(err)

    actual_kind = handle.descriptor.kind
    if actual_kind != expected_kind:
        err = Error(
            code=Errs.REF_KIND_MISMATCH,
            source="core.dependency",
            route=route,
            evidence={
                "expected_kind": expected_kind,
                "actual_kind": actual_kind,
                "ref_id": handle.ref_id,
            },
        )
        raise RefResolutionError(err)
    return handle


__all__ = [
    "ArgParam",
    "CtxParam",
    "Dependent",
    "Param",
    "ParameterInfo",
    "RefParam",
    "RefResolutionError",
    "ServiceParam",
    "UnresolvedParameterError",
]
