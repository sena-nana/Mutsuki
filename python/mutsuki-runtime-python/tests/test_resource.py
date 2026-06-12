from __future__ import annotations

import pytest

from mutsuki_runtime_python.backend import BackendInvokeError
from mutsuki_runtime_python.contracts import (
    ERR_REF_NOT_FOUND,
    LeaseToken,
    RefDescriptor,
    to_json_dict,
)
from mutsuki_runtime_python.resource import CounterIdSource, PythonResourceBackend


def _descriptor() -> RefDescriptor:
    return RefDescriptor(
        ref_id="ref-1",
        kind="domain.resource",
        schema_id_target="domain.resource",
        schema_version_target="1.0.0",
        attributes={"kind": "test"},
        lineage=(),
    )


async def test_python_resource_backend_tracks_descriptor_leases_without_handle() -> None:
    backend = PythonResourceBackend(CounterIdSource())

    ref_id = await backend.register_resource(_descriptor(), owner="resource-host")
    lease = await backend.acquire_resource(ref_id, requester="agent-a")
    records = backend.list_records()

    assert lease == LeaseToken(token_id="lease-0", ref_id="ref-1", owner="agent-a")
    assert records[0].descriptor == _descriptor()
    assert records[0].lease_count == 1
    assert "handle" not in to_json_dict(records[0])

    await backend.release_resource(lease)
    assert backend.list_records()[0].lease_count == 0


async def test_python_resource_backend_unknown_ref_fails_structured() -> None:
    backend = PythonResourceBackend(CounterIdSource())

    with pytest.raises(BackendInvokeError) as exc_info:
        await backend.acquire_resource("missing", requester="agent-a")

    assert exc_info.value.error.code == ERR_REF_NOT_FOUND
    assert exc_info.value.error.evidence["ref_id"] == "missing"


async def test_python_resource_backend_rejects_forged_lease_token() -> None:
    backend = PythonResourceBackend(CounterIdSource())
    ref_id = await backend.register_resource(_descriptor(), owner="resource-host")
    lease = await backend.acquire_resource(ref_id, requester="agent-a")
    forged = LeaseToken(
        token_id=lease.token_id,
        ref_id="ref-other",
        owner="agent-b",
    )

    with pytest.raises(BackendInvokeError) as exc_info:
        await backend.release_resource(forged)

    assert exc_info.value.error.code == ERR_REF_NOT_FOUND
    assert exc_info.value.error.evidence["reason"] == "lease_token_mismatch"
    assert backend.list_records()[0].lease_count == 1

    await backend.release_resource(lease)
    assert backend.list_records()[0].lease_count == 0
