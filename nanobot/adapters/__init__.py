"""Adapter 把外部传输映射到 NanoBot 的内部 Message 契约。"""

from nanobot.adapters.base import Adapter, AdapterCapability
from nanobot.adapters.inmemory import InMemoryAdapter

__all__ = ["Adapter", "AdapterCapability", "InMemoryAdapter"]
