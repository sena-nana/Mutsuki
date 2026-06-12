from __future__ import annotations

from typing import Protocol

from mutsuki_runtime_python.backend import BackendInvokeError
from mutsuki_runtime_python.contracts import (
    ERR_REF_NOT_FOUND,
    LeaseToken,
    RefDescriptor,
    ResourceRecord,
    RuntimeError,
)


class IdSource(Protocol):
    def next(self, prefix: str) -> str: ...


class CounterIdSource:
    def __init__(self) -> None:
        self._next = 0

    def next(self, prefix: str) -> str:
        token = f"{prefix}-{self._next}"
        self._next += 1
        return token


class PythonResourceBackend:
    """Descriptor-only resource backend with injected lease-token IDs."""

    def __init__(self, id_source: IdSource | None = None) -> None:
        self._id_source = id_source or CounterIdSource()
        self._records: dict[str, ResourceRecord] = {}
        self._leases: dict[str, LeaseToken] = {}

    async def register_resource(self, descriptor: RefDescriptor, owner: str) -> str:
        self._records[descriptor.ref_id] = ResourceRecord(
            descriptor=descriptor,
            owner=owner,
            lease_count=0,
        )
        return descriptor.ref_id

    async def acquire_resource(self, ref_id: str, requester: str) -> LeaseToken:
        record = self._records.get(ref_id)
        if record is None:
            raise BackendInvokeError(
                RuntimeError(
                    code=ERR_REF_NOT_FOUND,
                    source="python_resource_backend",
                    route=f"python.resource.acquire.{ref_id}",
                    evidence={"ref_id": ref_id, "requester": requester},
                )
            )
        token = LeaseToken(
            token_id=self._id_source.next("lease"),
            ref_id=ref_id,
            owner=requester,
        )
        self._leases[token.token_id] = token
        self._records[ref_id] = ResourceRecord(
            descriptor=record.descriptor,
            owner=record.owner,
            lease_count=record.lease_count + 1,
        )
        return token

    async def release_resource(self, token: LeaseToken) -> None:
        stored = self._leases.get(token.token_id)
        if stored is None:
            raise BackendInvokeError(
                RuntimeError(
                    code=ERR_REF_NOT_FOUND,
                    source="python_resource_backend",
                    route=f"python.resource.release.{token.token_id}",
                    evidence={"token_id": token.token_id, "ref_id": token.ref_id},
                )
            )
        if stored != token:
            raise BackendInvokeError(
                RuntimeError(
                    code=ERR_REF_NOT_FOUND,
                    source="python_resource_backend",
                    route=f"python.resource.release.{token.token_id}",
                    evidence={
                        "reason": "lease_token_mismatch",
                        "token_id": token.token_id,
                        "expected_ref_id": stored.ref_id,
                        "actual_ref_id": token.ref_id,
                        "expected_owner": stored.owner,
                        "actual_owner": token.owner,
                    },
                )
            )
        self._leases.pop(token.token_id)
        record = self._records.get(stored.ref_id)
        if record is None:
            return
        self._records[stored.ref_id] = ResourceRecord(
            descriptor=record.descriptor,
            owner=record.owner,
            lease_count=max(0, record.lease_count - 1),
        )

    def list_records(self, owner: str | None = None) -> tuple[ResourceRecord, ...]:
        records = self._records.values()
        if owner is not None:
            records = [record for record in records if record.owner == owner]
        return tuple(sorted(records, key=lambda record: record.descriptor.ref_id))


__all__ = ["CounterIdSource", "IdSource", "PythonResourceBackend"]
