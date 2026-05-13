"""服务容器 —— 按 ``(contract, name)`` 解析。

容器只负责按契约类型 + 可选名字解析实例。``ServiceMode`` 是 manifest 层的
契约元数据（参见 :class:`nanobot.contracts.plugin.ServiceDep`），与运行时
解析无关 —— 跨进程序列化拦截在 v0.2 的 codec 层做，那时再把 mode 接回来。
"""

from __future__ import annotations

from typing import Any


class ServiceNotFoundError(KeyError):
    pass


class ServiceContainer:
    """进程或 Agent 作用域的服务容器。"""

    def __init__(self) -> None:
        self._by_type: dict[type, list[tuple[str | None, Any]]] = {}

    def register(
        self,
        contract: type,
        instance: Any,
        *,
        name: str | None = None,
    ) -> None:
        self._by_type.setdefault(contract, []).append((name, instance))

    def unregister(self, contract: type, instance: Any) -> None:
        bucket = self._by_type.get(contract)
        if not bucket:
            return
        self._by_type[contract] = [t for t in bucket if t[1] is not instance]

    def resolve(self, contract: type, *, name: str | None = None) -> Any:
        bucket = self._by_type.get(contract)
        if not bucket:
            raise ServiceNotFoundError(
                f"契约 {contract!r} 没有已注册的服务"
            )
        if name is not None:
            for n, inst in bucket:
                if n == name:
                    return inst
            raise ServiceNotFoundError(
                f"契约 {contract!r} 下没有名为 {name!r} 的服务"
            )
        return bucket[0][1]

    def has(self, contract: type, *, name: str | None = None) -> bool:
        bucket = self._by_type.get(contract)
        if not bucket:
            return False
        if name is None:
            return True
        return any(n == name for n, _i in bucket)


__all__ = ["ServiceContainer", "ServiceNotFoundError"]
