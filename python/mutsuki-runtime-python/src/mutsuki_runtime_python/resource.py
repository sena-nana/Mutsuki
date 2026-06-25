from __future__ import annotations

import json
import tempfile
from pathlib import Path

from mutsuki_runtime_python.contracts import (
    ERR_CAPABILITY_EXHAUSTED,
    ERR_RESOURCE_GENERATION_MISMATCH,
    ERR_RESOURCE_LEASE_EXPIRED,
    ERR_RESOURCE_NOT_FOUND,
    JsonValue,
    LeaseToken,
    ResourceAccess,
    ResourceLifetime,
    ResourceRef,
    ResourceSealState,
    RuntimeError,
    ValueRef,
    ValueStorage,
)
from mutsuki_runtime_python.runner import RunnerInvokeError


class PythonResourceManager:
    def __init__(self, inline_value_max_bytes: int = 4096) -> None:
        self.inline_value_max_bytes = inline_value_max_bytes
        self._next = 0
        self._values: dict[str, tuple[ValueRef, JsonValue]] = {}
        self._resources: dict[str, tuple[ResourceRef, bytes, LeaseToken | None]] = {}
        self._root = Path(tempfile.gettempdir()) / "mutsuki-python-resource-manager"
        self._root.mkdir(parents=True, exist_ok=True)

    def pack_value(self, schema: str, value: JsonValue) -> JsonValue | ValueRef:
        encoded = json.dumps(value, separators=(",", ":"), ensure_ascii=False).encode()
        if len(encoded) <= self.inline_value_max_bytes:
            return value
        ref_id = self._id("value")
        value_ref = ValueRef(
            ref_id=ref_id,
            provider_id="python.resource",
            schema=schema,
            version=1,
            generation=1,
            size_hint=len(encoded),
            content_hash=_simple_hash(encoded),
            lifetime=ResourceLifetime.PERSISTENT,
            storage=ValueStorage.LOCAL_VALUE_STORE,
        )
        self._values[ref_id] = (value_ref, value)
        return value_ref

    def get_value(self, value_ref: ValueRef) -> JsonValue:
        stored = self._values.get(value_ref.ref_id)
        if stored is None:
            raise _resource_error(ERR_RESOURCE_NOT_FOUND, f"value.{value_ref.ref_id}")
        stored_ref, value = stored
        if stored_ref.generation != value_ref.generation:
            raise _resource_error(ERR_RESOURCE_GENERATION_MISMATCH, f"value.{value_ref.ref_id}")
        return value

    def create_mmap_resource(self, schema: str, data: bytes) -> ResourceRef:
        ref_id = self._id("resource")
        path = self._root / f"{ref_id}.bin"
        path.write_bytes(data)
        resource = ResourceRef(
            ref_id=ref_id,
            provider_id="python.resource",
            resource_kind="bytes",
            schema=schema,
            version=1,
            generation=1,
            access=ResourceAccess.mmap_file(
                path=str(path),
                offset=0,
                len=len(data),
                readonly=True,
            ),
            size_hint=len(data),
            content_hash=_simple_hash(data),
            lifetime=ResourceLifetime.PERSISTENT,
            lease=None,
            seal_state=ResourceSealState.SEALED,
        )
        return self._store_resource(resource, data)

    def create_blob_resource(self, schema: str, data: bytes) -> ResourceRef:
        ref_id = self._id("resource")
        resource = ResourceRef(
            ref_id=ref_id,
            provider_id="python.resource",
            resource_kind="blob",
            schema=schema,
            version=1,
            generation=1,
            access=ResourceAccess.blob(store_id="python.resource.blob", key=ref_id),
            size_hint=len(data),
            content_hash=_simple_hash(data),
            lifetime=ResourceLifetime.PERSISTENT,
            lease=None,
            seal_state=ResourceSealState.SEALED,
        )
        return self._store_resource(resource, data)

    def read_resource(self, resource_ref: ResourceRef) -> bytes:
        stored = self._resources.get(resource_ref.ref_id)
        if stored is None:
            raise _resource_error(ERR_RESOURCE_NOT_FOUND, f"resource.{resource_ref.ref_id}")
        current, data, _lease = stored
        if current.generation != resource_ref.generation:
            raise _resource_error(
                ERR_RESOURCE_GENERATION_MISMATCH, f"resource.{resource_ref.ref_id}"
            )
        if current.access.path is not None:
            return Path(current.access.path).read_bytes()
        return data

    def copy_on_write(self, base_ref: ResourceRef, data: bytes) -> ResourceRef:
        self.read_resource(base_ref)
        return self.create_mmap_resource(base_ref.schema, data)

    def acquire_write_lease(
        self,
        ref_id: str,
        owner: str,
        expires_at_step: int | None = None,
    ) -> LeaseToken:
        stored = self._resources.get(ref_id)
        if stored is None:
            raise _resource_error(ERR_RESOURCE_NOT_FOUND, f"resource.lease.{ref_id}")
        resource, data, lease = stored
        if lease is not None:
            raise _resource_error(ERR_CAPABILITY_EXHAUSTED, f"resource.lease.{ref_id}")
        token = LeaseToken(
            token_id=self._id("lease"),
            ref_id=ref_id,
            owner=owner,
            mode="exclusive_write",
            expires_at_step=expires_at_step,
            generation=resource.generation,
        )
        self._resources[ref_id] = (resource, data, token)
        return token

    def write_with_lease(self, token: LeaseToken, data: bytes, current_step: int) -> ResourceRef:
        if token.expires_at_step is not None and current_step > token.expires_at_step:
            raise _resource_error(ERR_RESOURCE_LEASE_EXPIRED, f"resource.write.{token.ref_id}")
        stored = self._resources.get(token.ref_id)
        if stored is None:
            raise _resource_error(ERR_RESOURCE_NOT_FOUND, f"resource.write.{token.ref_id}")
        resource, _old_data, lease = stored
        if lease != token:
            raise _resource_error(
                ERR_RESOURCE_GENERATION_MISMATCH, f"resource.write.{token.ref_id}"
            )
        path = resource.access.path
        if path is not None:
            Path(path).write_bytes(data)
        updated = ResourceRef(
            ref_id=resource.ref_id,
            provider_id=resource.provider_id,
            resource_kind=resource.resource_kind,
            schema=resource.schema,
            version=resource.version + 1,
            generation=resource.generation + 1,
            access=resource.access,
            size_hint=len(data),
            content_hash=_simple_hash(data),
            lifetime=resource.lifetime,
            lease=None,
            seal_state=ResourceSealState.SEALED,
        )
        self._resources[token.ref_id] = (updated, data, None)
        return updated

    def _id(self, prefix: str) -> str:
        self._next += 1
        return f"{prefix}-{self._next:08d}"

    def _store_resource(self, resource: ResourceRef, data: bytes) -> ResourceRef:
        self._resources[resource.ref_id] = (resource, data, None)
        return resource


def _resource_error(code: str, route: str) -> RunnerInvokeError:
    return RunnerInvokeError(RuntimeError(code=code, source="python_resource_manager", route=route))


def _simple_hash(data: bytes) -> str:
    return f"sum:{sum(data)}:len:{len(data)}"
