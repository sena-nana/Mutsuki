from __future__ import annotations

from mutsuki_runner_kit.contracts.batch import PayloadLayout
from mutsuki_runner_kit.contracts.entry import (
    DispatchLane,
    OrderingRequirement,
    ResourceAccessMode,
    ResourceRequirement,
)
from mutsuki_runner_kit.contracts.resource import (
    ResourceAccess,
    ResourceId,
    ResourceLifetime,
    ResourceRef,
    ResourceSealState,
    ResourceSemantic,
    ValueRef,
    ValueStorage,
)
from mutsuki_runner_kit.contracts.runner import (
    ExecutionClass,
    InvocationMode,
    RunnerBatchCapability,
    RunnerConcurrency,
    RunnerContext,
    RunnerControlCapability,
    RunnerDescriptor,
    RunnerMode,
    RunnerOrderingCapability,
    RunnerPayloadCapability,
    RunnerPurity,
    RunnerResourceCapability,
    RunnerResult,
    RunnerSideEffect,
    RunnerStatus,
    TimeoutGranularity,
)
from mutsuki_runner_kit.contracts.state import VersionExpectation
from mutsuki_runner_kit.contracts.task import (
    CancelPolicy,
    Task,
    TaskAwait,
    TaskHandle,
    TaskLease,
    TaskOutcome,
    TaskStepContinuation,
    WakeCondition,
)
from mutsuki_runner_kit.testing.assertions import assert_json_roundtrip


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
        dispatch_lane=DispatchLane.INTERACTIVE,
        ordering=OrderingRequirement.preserve_submit_order(),
        resource_requirements=(
            ResourceRequirement(
                ref_id="resource:1",
                mode=ResourceAccessMode.READ,
                expected_version=1,
            ),
        ),
        created_sequence=4,
    )
    assert_json_roundtrip(Task, task)

    descriptor = RunnerDescriptor(
        runner_id="runner-a",
        plugin_id="plugin-a",
        plugin_generation=1,
        accepted_protocol_ids=("raw.input",),
        purity=RunnerPurity.PURE,
        execution_class=ExecutionClass.CPU,
        invocation_mode=InvocationMode.ASYNC_REENTRANT,
        concurrency=RunnerConcurrency.reentrant(4, 32),
        input_schema={"type": "object"},
        output_schema={"type": "object"},
        batch=RunnerBatchCapability(
            mode=RunnerMode.NATIVE_BATCH,
            preferred_batch_size=64,
            max_batch_entries=256,
            max_entry_concurrency=8,
            max_inflight_batches=1,
            scalar_thread_safe=True,
            scalar_reentrant=True,
            partial_failure=True,
            preserve_order=False,
            side_effect=RunnerSideEffect.RESOURCE,
        ),
        payload=RunnerPayloadCapability(
            layouts=(PayloadLayout.ROW, PayloadLayout.COLUMNAR),
            preferred_layout=PayloadLayout.COLUMNAR,
            zero_copy=True,
        ),
        resources=RunnerResourceCapability(
            batch_read=True,
            batch_write=True,
            requires_resource_plan=True,
            supports_shared_memory=True,
        ),
        ordering=RunnerOrderingCapability(
            default=OrderingRequirement.none(),
            supports_sequence=True,
            supports_same_resource_order=True,
        ),
        control=RunnerControlCapability(
            entry_cancel=True,
            batch_cancel=True,
            timeout_granularity=TimeoutGranularity.ENTRY,
        ),
        metadata={"rank": 1},
        contract_surfaces=("runner:runner-a",),
    )
    assert_json_roundtrip(RunnerDescriptor, descriptor)
    assert_json_roundtrip(
        RunnerContext,
        RunnerContext(
            registry_generation=1,
            current_step=2,
            tick_id="tick-2",
            batch_id="batch-1",
            executor_id="executor-a",
            task_lease_ids=("lease-1", "lease-2"),
            entry_count=2,
            invocation_id="inv-1",
            cancel_token="inv-1",
            deadline_tick=9,
            deadline_after_ms=250,
            cancel_requested=False,
        ),
    )
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
            attempt_generation=2,
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
        resource_id=ResourceId(kind_id="bytes", slot_id="resource:1", generation=1, version=1),
        semantic=ResourceSemantic.FROZEN_VALUE,
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
        output={"answer": 42},
        values=(value_ref,),
        resources=(resource_ref,),
    )

    assert_json_roundtrip(RunnerResult, result)


def test_task_handle_outcome_and_await_roundtrip() -> None:
    handle = TaskHandle(
        task_id="child-1",
        protocol_id="child.work",
        target_binding_id=None,
        cancel_policy=CancelPolicy.CASCADE,
        trace_id="trace-1",
        correlation_id="corr-1",
    )
    assert_json_roundtrip(TaskHandle, handle)
    assert_json_roundtrip(
        TaskOutcome,
        TaskOutcome.completed("child-1", "value:child", output={"answer": 42}),
    )

    continuation = TaskStepContinuation(
        continuation=ResourceRef(
            ref_id="continuation:parent-1",
            resource_id=ResourceId(
                kind_id="continuation",
                slot_id="continuation:parent-1",
                generation=1,
                version=1,
            ),
            semantic=ResourceSemantic.FROZEN_VALUE,
            provider_id="mutsuki.sdk",
            resource_kind="continuation",
            schema="mutsuki.continuation.v1",
            version=1,
            generation=1,
            access=ResourceAccess.inline(),
            size_hint=None,
            content_hash=None,
            lifetime=ResourceLifetime.BORROWED_UNTIL_TASK_END,
            lease=None,
            seal_state=ResourceSealState.SEALED,
        ),
        wake=WakeCondition.manual_wake(),
        reason="sdk.await",
    )
    task_await = TaskAwait(
        parent_task_id="parent-1",
        child=handle,
        continuation=continuation,
        cancel_policy=CancelPolicy.CASCADE,
    )
    assert_json_roundtrip(TaskAwait, task_await)
    assert_json_roundtrip(
        RunnerResult,
        RunnerResult(task_id="parent-1", task_await=task_await, status=RunnerStatus.WAITING),
    )
