"""Branded string identities that serialize as JSON strings."""

from __future__ import annotations

__all__ = (
    "BatchId",
    "BatchKey",
    "BindingId",
    "CapabilityPeerId",
    "CapabilityRequestId",
    "EntryId",
    "ExecutorId",
    "PluginId",
    "ProtocolId",
    "RefId",
    "ResourceCellId",
    "ResourceLeaseId",
    "RunnerId",
    "SpanId",
    "SurfaceId",
    "TaskId",
    "TaskLeaseId",
    "TickId",
    "TraceId",
)


class TaskId(str):
    __slots__ = ()


class RefId(str):
    __slots__ = ()


class ProtocolId(str):
    __slots__ = ()


class RunnerId(str):
    __slots__ = ()


class PluginId(str):
    __slots__ = ()


class ExecutorId(str):
    __slots__ = ()


class BindingId(str):
    __slots__ = ()


class TaskLeaseId(str):
    __slots__ = ()


class ResourceLeaseId(str):
    __slots__ = ()


class ResourceCellId(str):
    __slots__ = ()


class TickId(str):
    __slots__ = ()


class BatchId(str):
    __slots__ = ()


class EntryId(str):
    __slots__ = ()


class BatchKey(str):
    __slots__ = ()


class SurfaceId(str):
    __slots__ = ()


class SpanId(str):
    __slots__ = ()


class TraceId(str):
    __slots__ = ()


class CapabilityRequestId(str):
    __slots__ = ()


class CapabilityPeerId(str):
    __slots__ = ()
