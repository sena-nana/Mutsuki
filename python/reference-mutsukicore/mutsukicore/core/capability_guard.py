"""Capability 静态准入检查。

如果命令的 ``requires_capabilities`` 不是其所属插件已声明 capability 的子集，
则拒绝调用，抛 :data:`Errs.CAPABILITY_NOT_DECLARED`。
"""

from __future__ import annotations

from mutsukicore.contracts.capability import CapabilityName
from mutsukicore.contracts.error import Error, Errs


class CapabilityNotDeclaredError(Exception):
    def __init__(self, missing: tuple[CapabilityName, ...], err: Error) -> None:
        super().__init__(f"未声明的 capability: {missing!r}")
        self.missing = missing
        self.error = err


def check_capabilities(
    *,
    plugin_id: str,
    declared: tuple[CapabilityName, ...],
    required: tuple[CapabilityName, ...],
    route: str,
) -> None:
    declared_set = set(declared)
    missing = tuple(c for c in required if c not in declared_set)
    if missing:
        err = Error(
            code=Errs.CAPABILITY_NOT_DECLARED,
            source=plugin_id,
            route=route,
            evidence={"missing": ",".join(missing)},
        )
        raise CapabilityNotDeclaredError(missing, err)


__all__ = ["CapabilityNotDeclaredError", "check_capabilities"]
