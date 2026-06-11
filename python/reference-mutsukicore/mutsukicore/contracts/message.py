"""Deprecated IM message compatibility shim.

Use :mod:`mutsukicore_ext.im` for IM contracts. Core contracts no longer own
``Message`` / ``ChannelRef`` / ``ContentPart``.
"""

from mutsukicore_ext.im import ChannelRef, ContentKind, ContentPart, Message

__all__ = ["ChannelRef", "ContentKind", "ContentPart", "Message"]
