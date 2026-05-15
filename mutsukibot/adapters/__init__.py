"""DEPRECATED —— Adapter 抽象在 v0.2 删除（详 D1 / contracts §14-§18）。

原 ``Adapter`` / ``AdapterCapability`` / ``InMemoryAdapter`` 被以下机制取代：

* Transport 翻译职责由 reference plugin 承担（参见
  :mod:`mutsukibot.plugins.inmemory_endpoint`）
* Source / Operation 通过 ``ctx.dispatch.register_source`` /
  ``register_operation`` 注册（参见 :mod:`mutsukibot.core.dispatcher`）
* Adapter capabilities StrEnum 改为注册式 ``CapabilityName``，命名空间下移
  到 ``Caps.IM_*`` / ``Caps.TOOL_*``

本 ``__init__.py`` 仅保留作为兼容 shim 的"墓碑" —— 任何对
``mutsukibot.adapters.Adapter`` / ``InMemoryAdapter`` 的 import 都会立即
``ImportError``，并指向迁移路径。
"""

from __future__ import annotations

_DEPRECATION_MESSAGE = (
    "mutsukibot.adapters has been removed in v0.2. "
    "Adapter -> Plugin (register_source + register_operation via ctx.dispatch). "
    "InMemoryAdapter -> mutsukibot.plugins.inmemory_endpoint.InMemoryEndpointPlugin. "
    "AdapterCapability StrEnum -> Caps.IM_* / Caps.TOOL_* (registered CapabilityName). "
    "See plans/contracts.md §14-§18 and AGENTS.md hard rule #14."
)


def __getattr__(name: str) -> object:
    raise ImportError(f"{_DEPRECATION_MESSAGE} (requested attr: {name!r})")


__all__: list[str] = []
