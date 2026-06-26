from __future__ import annotations

from mutsuki_runtime_python.contracts.resource import (
    ResourceAccess,
    ResourceLifetime,
    ResourceRef,
    ResourceSealState,
    ValueRef,
    ValueStorage,
)
from mutsuki_runtime_python.contracts.runner import (
    RunnerDescriptor,
    RunnerPurity,
    RunnerResult,
)
from mutsuki_runtime_python.contracts.state import VersionExpectation
from mutsuki_runtime_python.contracts.task import Task, TaskLease
from mutsuki_runtime_python.testing.assertions import assert_json_roundtrip


def test_task_and_runner_descriptor_roundtrip() -> None:
    task = Task(
        task_id="task-1",
        protocol_id="raw.input",
        priority=10,
        ready_at_step=2,
        payload={"actor_id": "actor-a"},
        input_refs=("value:raw-1",),
        output_ref=None,
        continuation_ref=None,
        target_binding_id="binding:raw",
        lease_id="task-lease-1",
        trace_id="trace-1",
        expected_versions=(VersionExpectation(ref_id="state:actor", expected_version=1),),
        correlation_id="corr-1",
        idempotency_key="idem-1",
        runner_hint="runner-a",
        registry_generation=3,
        required_surfaces=("task_protocol:raw.input",),
        created_sequence=4,
    )
    assert_json_roundtrip(Task, task)

    descriptor = RunnerDescriptor(
        runner_id="runner-a",
        plugin_id="plugin-a",
        plugin_generation=1,
        accepted_protocol_ids=("raw.input",),
        purity=RunnerPurity.PURE,
        input_schema={"type": "object"},
        output_schema={"type": "object"},
        metadata={"rank": 1},
        contract_surfaces=("runner:runner-a",),
    )
    assert_json_roundtrip(RunnerDescriptor, descriptor)
    assert_json_roundtrip(
        TaskLease,
        TaskLease(
            lease_id="task-lease-1",
            task_id="task-1",
            runner_id="runner-a",
            executor_id="executor-a",
            registry_generation=3,
            acquired_at_step=2,
            expires_at_step=None,
        ),
    )


def test_runner_result_roundtrips_value_and_resource_refs() -> None:
    value_ref = ValueRef(
        ref_id="value:1",
        provider_id="python.resource",
        schema="value.small.v1",
        version=1,
        generation=1,
        size_hint=12,
        content_hash="hash:value",
        lifetime=ResourceLifetime.PERSISTENT,
        storage=ValueStorage.LOCAL_VALUE_STORE,
    )
    resource_ref = ResourceRef(
        ref_id="resource:1",
        provider_id="python.resource",
        resource_kind="bytes",
        schema="bytes.v1",
        version=1,
        generation=1,
        access=ResourceAccess.mmap_file(
            path="resource.bin",
            offset=0,
            len=3,
            readonly=True,
        ),
        size_hint=3,
        content_hash="hash:resource",
        lifetime=ResourceLifetime.PERSISTENT,
        lease=None,
        seal_state=ResourceSealState.SEALED,
    )
    result = RunnerResult(
        task_id="task-1",
        values=(value_ref,),
        resources=(resource_ref,),
    )

    assert_json_roundtrip(RunnerResult, result)
