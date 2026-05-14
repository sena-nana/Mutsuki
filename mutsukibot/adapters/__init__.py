"""Adapter 把外部传输映射到 MutsukiBot 的内部 Message 契约。"""

from mutsukibot.adapters.base import Adapter, AdapterCapability
from mutsukibot.adapters.inmemory import InMemoryAdapter

__all__ = ["Adapter", "AdapterCapability", "InMemoryAdapter"]
