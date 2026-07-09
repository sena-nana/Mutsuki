from __future__ import annotations

import importlib
from typing import ClassVar

import pytest

from mutsuki_runner_kit.contracts.resource import ResourceSemantic, ValueRef
from mutsuki_runner_kit.resources import ResourceClient, ResourceKind
from mutsuki_runner_kit.runners.protocol import RunnerInvokeError

PythonResourceManager = importlib.import_module(
    "mutsuki_runner_kit." + "resources.manager"
).PythonResourceManager


def resource_kind(kind_id: str, semantic: ResourceSemantic) -> type[ResourceKind]:
    class Marker:
        KIND_ID: ClassVar[str] = kind_id
        SEMANTIC: ClassVar[ResourceSemantic] = semantic

    return Marker


def test_resource_manager_packs_small_and_large_values() -> None:
    manager = PythonResourceManager(inline_value_max_bytes=16)

    assert manager.pack_value("small.v1", {"a": 1}) == {"a": 1}
    large = manager.pack_value("large.v1", {"blob": "x" * 100})

    assert isinstance(large, ValueRef)
    assert manager.get_value(large) == {"blob": "x" * 100}


def test_resource_manager_supports_mmap_cow_and_exclusive_write_lease() -> None:
    manager = PythonResourceManager()
    resource = manager.create_mmap_resource("bytes.v1", b"abc")

    assert manager.read_resource(resource) == b"abc"
    blob = manager.create_blob_resource("blob.v1", b"blob-data")
    assert blob.access.type == "blob"
    assert manager.read_resource(blob) == b"blob-data"
    cow = manager.copy_on_write(resource, b"xyz")
    assert cow.ref_id != resource.ref_id
    assert cow.semantic == ResourceSemantic.COW_VERSIONED_STATE
    lease = manager.acquire_write_lease(resource.ref_id, "runner-a", expires_at_step=5)
    updated = manager.write_with_lease(lease, b"def", current_step=2)

    assert updated.generation == resource.generation + 1
    assert manager.read_resource(updated) == b"def"


def test_expired_write_lease_fails_loudly() -> None:
    manager = PythonResourceManager()
    resource = manager.create_mmap_resource("bytes.v1", b"abc")
    lease = manager.acquire_write_lease(resource.ref_id, "runner-a", expires_at_step=1)

    with pytest.raises(RunnerInvokeError):
        manager.write_with_lease(lease, b"late", current_step=2)


def test_resource_manager_supports_typed_resources_and_lazy_plans() -> None:
    manager = PythonResourceManager()
    text = manager.create_cow_state_resource("text_buffer", "text.v1", b"hello")
    ast = manager.create_snapshot_resource("ast_snapshot", "ast.v1", text, b"ast")
    facts = manager.create_fact_resource("project_facts", "facts.v1", {"root": "."})
    stream = manager.create_stream_resource("model_output_stream", "token.v1", "stream://model")
    capability = manager.create_capability_resource("db_pool", "db.pool.v1")

    assert text.semantic == ResourceSemantic.COW_VERSIONED_STATE
    assert ast.semantic == ResourceSemantic.VERSIONED_SNAPSHOT
    assert facts.semantic == ResourceSemantic.READ_ONLY_FACT
    assert stream.semantic == ResourceSemantic.STREAM_RESOURCE
    assert capability.semantic == ResourceSemantic.CAPABILITY_RESOURCE

    read_plan = manager.build_read_plan(text, "collect")
    write_plan = manager.build_write_plan(text, "fail", {"replace": "all"})

    assert manager.read_resource(text) == b"hello"
    assert manager.collect_read_plan(read_plan) == b"hello"
    receipt = manager.commit_write_plan(write_plan, b"world")

    assert receipt.new_version == 2
    assert receipt.resource_ref is not None
    assert receipt.descriptor_updates == (receipt.resource_ref,)
    assert manager.read_resource(receipt.resource_ref) == b"world"
    assert manager.open_stream_plan(manager.build_read_plan(stream, "open")).resource == stream

    export = manager.export_plan(text, "json")
    command = manager.command_plan(capability, "query", {"sql": "select 1"}, "query:1")
    transaction = manager.transaction_plan("tx:1", (write_plan,), strict=True)
    batch = manager.command_batch("batch:1", (command,), rollback_guarantee=False)
    saga = manager.saga_plan("saga:1", (command,), (command,))

    assert export.target == "json"
    assert command.capability == capability
    assert command.idempotency_key == "query:1"
    assert transaction.operations == (write_plan,)
    assert batch.commands == (command,)
    assert saga.compensations == (command,)


def test_resource_client_builds_issue_9_example_resource_plans() -> None:
    manager = PythonResourceManager()
    client = ResourceClient()
    text = client.handle(
        manager.create_cow_state_resource("text_buffer", "text.v1", b"hello"),
        resource_kind("text_buffer", ResourceSemantic.COW_VERSIONED_STATE),
    )
    ast = client.handle(
        manager.create_snapshot_resource("ast_snapshot", "ast.v1", text.resource, b"ast"),
        resource_kind("ast_snapshot", ResourceSemantic.VERSIONED_SNAPSHOT),
    )
    facts = client.handle(
        manager.create_fact_resource("project_facts", "facts.v1", {"root": "."}),
        resource_kind("project_facts", ResourceSemantic.READ_ONLY_FACT),
    )
    stream = client.handle(
        manager.create_stream_resource("model_output_stream", "token.v1", "stream://model"),
        resource_kind("model_output_stream", ResourceSemantic.STREAM_RESOURCE),
    )
    db = client.handle(
        manager.create_capability_resource("db_pool", "db.pool.v1"),
        resource_kind("db_pool", ResourceSemantic.CAPABILITY_RESOURCE),
    )

    assert text.descriptor_matches_kind()
    assert ast.descriptor_matches_kind()
    assert facts.descriptor_matches_kind()
    assert stream.descriptor_matches_kind()
    assert db.descriptor_matches_kind()

    write = client.write_plan(text, "fail", {"replace": "all"})
    assert manager.commit_write_plan(write, b"world").new_version == 2
    assert client.read_plan(ast, "collect").resource.semantic == ResourceSemantic.VERSIONED_SNAPSHOT
    assert client.read_plan(facts, "query").resource.semantic == ResourceSemantic.READ_ONLY_FACT
    assert client.stream_plan(stream).operation == "open_stream"
    assert client.command_plan(db, "query", {"sql": "select 1"}).operation == "query"


def test_resource_manager_executes_export_command_batch_and_saga_plans() -> None:
    manager = PythonResourceManager()
    text = manager.create_blob_resource("text.v1", b"hello")
    capability = manager.create_capability_resource("db_pool", "db.pool.v1")

    export = manager.export_plan(text, "inline_utf8")
    export_receipt = manager.execute_export_plan(export)
    assert export_receipt.status == "exported"
    assert export_receipt.output == "hello"

    command = manager.command_plan(capability, "query", {"sql": "select 1"}, "query:1")
    command_receipt = manager.execute_command_plan(command)
    assert command_receipt.status == "commanded"
    assert command_receipt.output["operation"] == "query"  # type: ignore[index]
    assert command_receipt.output["idempotency_key"] == "query:1"  # type: ignore[index]

    batch = manager.command_batch("batch:1", (command, command), rollback_guarantee=False)
    assert len(manager.execute_command_batch(batch)) == 2

    saga = manager.saga_plan("saga:1", (command,), (command,))
    assert len(manager.execute_saga_plan(saga)) == 1


def test_resource_manager_rejects_invalid_executable_plans_loudly() -> None:
    manager = PythonResourceManager()
    text = manager.create_blob_resource("text.v1", b"hello")
    binary = manager.create_blob_resource("bytes.v1", b"\xff")
    capability = manager.create_capability_resource("db_pool", "db.pool.v1")

    with pytest.raises(RunnerInvokeError) as unsupported_export:
        manager.execute_export_plan(manager.export_plan(text, "json"))
    assert unsupported_export.value.error.code == "resource.export_unsupported"

    with pytest.raises(RunnerInvokeError) as decode_failure:
        manager.execute_export_plan(manager.export_plan(binary, "inline_utf8"))
    assert decode_failure.value.error.code == "resource.export_decode_failed"

    with pytest.raises(RunnerInvokeError) as semantic_failure:
        manager.execute_command_plan(manager.command_plan(text, "query", None))
    assert semantic_failure.value.error.code == "resource.semantic_mismatch"

    command = manager.command_plan(capability, "query", {"sql": "select 1"})
    rollback_batch = manager.command_batch("batch:1", (command,), rollback_guarantee=True)
    with pytest.raises(RunnerInvokeError) as rollback_failure:
        manager.execute_command_batch(rollback_batch)
    assert rollback_failure.value.error.code == "resource.rollback_unsupported"

    unsupported_command = manager.command_plan(capability, "drop", None)
    with pytest.raises(RunnerInvokeError) as saga_failure:
        manager.execute_saga_plan(
            manager.saga_plan("saga:failed", (unsupported_command,), (command,))
        )
    assert saga_failure.value.error.code == "resource.saga_failed"
    assert saga_failure.value.error.cause is not None
    assert saga_failure.value.error.cause.code == "resource.command_unsupported"
    assert saga_failure.value.error.evidence["compensation_attempts"] == 1


def test_stale_write_plan_fails_loudly() -> None:
    manager = PythonResourceManager()
    text = manager.create_cow_state_resource("text_buffer", "text.v1", b"hello")
    stale = manager.build_write_plan(text, "fail", {"replace": "old"})
    fresh = manager.build_write_plan(text, "fail", {"replace": "new"})

    manager.commit_write_plan(fresh, b"new")

    with pytest.raises(RunnerInvokeError) as stale_failure:
        manager.commit_write_plan(stale, b"old")
    assert stale_failure.value.error.code == "resource.generation_mismatch"
