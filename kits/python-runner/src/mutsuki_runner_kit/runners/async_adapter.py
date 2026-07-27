from __future__ import annotations

from collections.abc import Awaitable, Callable, Generator
from dataclasses import replace
from typing import Any, Protocol

from mutsuki_runner_kit.contracts.batch import CompletionBatch, WorkBatch
from mutsuki_runner_kit.contracts.codec import JsonValue
from mutsuki_runner_kit.contracts.errors import RuntimeError
from mutsuki_runner_kit.contracts.resource import (
    ResourceAccess,
    ResourceId,
    ResourceLifetime,
    ResourceRef,
    ResourceSealState,
    ResourceSemantic,
)
from mutsuki_runner_kit.contracts.runner import (
    RunnerContext,
    RunnerDescriptor,
    RunnerResult,
    RunnerStatus,
)
from mutsuki_runner_kit.contracts.task import (
    CancelPolicy,
    Task,
    TaskAwait,
    TaskHandle,
    TaskOutcome,
    TaskStepContinuation,
)
from mutsuki_runner_kit.runners.protocol import RunnerInvokeError
from mutsuki_runner_kit.runners.scalar import ScalarBatchAdapter


class RuntimeClient(Protocol):
    def task_outcome(self, task_id: str) -> TaskOutcome | None: ...


class AsyncRunnerContext:
    def __init__(
        self,
        client: RuntimeClient,
        parent_task: Task,
        current_runner_id: str,
        *,
        invocation_id: str = "",
        cancel_token: str = "",
        deadline_tick: int | None = None,
        deadline_after_ms: int | None = None,
        cancel_requested: bool = False,
        allow_self_call: bool = True,
    ) -> None:
        self._client = client
        self._parent_task = parent_task
        self._current_runner_id = current_runner_id
        self._invocation_id = invocation_id
        self._cancel_token = cancel_token
        self._deadline_tick = deadline_tick
        self._deadline_after_ms = deadline_after_ms
        self._cancel_requested = cancel_requested
        self._allow_self_call = allow_self_call
        self._next_call = 0

    @property
    def task_id(self) -> str:
        return self._parent_task.task_id

    @property
    def invocation_id(self) -> str:
        return self._invocation_id

    @property
    def cancel_token(self) -> str:
        return self._cancel_token

    @property
    def deadline_tick(self) -> int | None:
        return self._deadline_tick

    @property
    def deadline_after_ms(self) -> int | None:
        return self._deadline_after_ms

    @property
    def cancel_requested(self) -> bool:
        return self._cancel_requested

    def call_raw(self, protocol_id: str, payload: JsonValue = None) -> TaskCallAwaitable:
        return self.call_with_cancel_policy(protocol_id, payload, CancelPolicy.CASCADE)

    def call_targeted_raw(
        self,
        binding_id: str,
        protocol_id: str,
        runner_hint: str,
        payload: JsonValue = None,
    ) -> TaskCallAwaitable:
        return self.call_targeted_with_cancel_policy(
            binding_id,
            protocol_id,
            runner_hint,
            payload,
            CancelPolicy.CASCADE,
        )

    def call_with_cancel_policy(
        self,
        protocol_id: str,
        payload: JsonValue = None,
        cancel_policy: CancelPolicy = CancelPolicy.CASCADE,
    ) -> TaskCallAwaitable:
        return self._build_call(protocol_id, payload, None, None, cancel_policy)

    def call_targeted_with_cancel_policy(
        self,
        binding_id: str,
        protocol_id: str,
        runner_hint: str,
        payload: JsonValue = None,
        cancel_policy: CancelPolicy = CancelPolicy.CASCADE,
    ) -> TaskCallAwaitable:
        return self._build_call(protocol_id, payload, binding_id, runner_hint, cancel_policy)

    def _build_call(
        self,
        protocol_id: str,
        payload: JsonValue,
        binding_id: str | None,
        runner_hint: str | None,
        cancel_policy: CancelPolicy,
    ) -> TaskCallAwaitable:
        if runner_hint == self._current_runner_id and not self._allow_self_call:
            raise RunnerInvokeError(
                error=RuntimeError(
                    code="task.self_call_blocked",
                    source="python_async_runner",
                    route=f"task.await.{self._parent_task.task_id}",
                    evidence={"runner_id": self._current_runner_id},
                )
            )
        self._next_call += 1
        task_id = f"{self._parent_task.task_id}:call:{self._next_call}"
        task = replace(
            Task.new(task_id, protocol_id, payload),
            target_binding_id=binding_id,
            runner_hint=runner_hint,
            trace_id=self._parent_task.trace_id,
            correlation_id=self._parent_task.correlation_id,
        )
        handle = TaskHandle(
            task_id=task.task_id,
            protocol_id=task.protocol_id,
            target_binding_id=binding_id,
            cancel_policy=cancel_policy,
            trace_id=task.trace_id,
            correlation_id=task.correlation_id,
        )
        task_await = TaskAwait(
            parent_task_id=self._parent_task.task_id,
            child=handle,
            continuation=TaskStepContinuation(
                continuation=_continuation_ref(self._parent_task.task_id),
                wake=None,
                reason="sdk.await",
            ),
            cancel_policy=cancel_policy,
        )
        return TaskCallAwaitable(PendingCall(task=task, task_await=task_await))


TaskAwaitRunnerFactory = Callable[[AsyncRunnerContext, Task], Awaitable[RunnerResult]]


class TaskCallAwaitable:
    def __init__(self, pending: PendingCall) -> None:
        self.pending = pending

    def __await__(self) -> Generator[PendingCall, TaskOutcome, TaskOutcome]:
        outcome = yield self.pending
        return outcome


class PendingCall:
    def __init__(self, task: Task | None, task_await: TaskAwait) -> None:
        self.task = task
        self.task_await = task_await

    def without_task(self) -> PendingCall:
        return PendingCall(task=None, task_await=self.task_await)


class TaskAwaitRunnerAdapter:
    def __init__(
        self,
        descriptor: RunnerDescriptor,
        client: RuntimeClient,
        factory: TaskAwaitRunnerFactory,
        *,
        allow_self_call: bool = True,
    ) -> None:
        self._descriptor = descriptor
        self._client = client
        self._factory = factory
        self._allow_self_call = allow_self_call
        self._invocations: dict[str, _Invocation] = {}
        self._invocation_tasks: dict[str, str] = {}

    @property
    def descriptor(self) -> RunnerDescriptor:
        return self._descriptor

    async def run_one(self, ctx: RunnerContext, task: Task) -> RunnerResult:
        invocation = self._invocations.get(task.task_id)
        if invocation is None:
            runner_ctx = AsyncRunnerContext(
                self._client,
                task,
                self._descriptor.runner_id,
                invocation_id=ctx.invocation_id,
                cancel_token=ctx.cancel_token,
                deadline_tick=ctx.deadline_tick,
                deadline_after_ms=ctx.deadline_after_ms,
                cancel_requested=ctx.cancel_requested,
                allow_self_call=self._allow_self_call,
            )
            invocation = _Invocation(self._factory(runner_ctx, task).__await__())
            self._invocations[task.task_id] = invocation
        self._track_invocation(task.task_id, ctx.invocation_id)

        outcome: TaskOutcome | None = None
        if invocation.pending is not None:
            outcome = self._client.task_outcome(invocation.pending.task_await.child.task_id)
            if outcome is None:
                return _waiting_result(task.task_id, invocation.pending.without_task())

        try:
            yielded = invocation.iterator.send(outcome)
        except StopIteration as stop:
            self._remove_invocation(task.task_id)
            result = stop.value
            if not isinstance(result, RunnerResult):
                raise RunnerInvokeError(
                    RuntimeError(
                        code="runner.invalid_result",
                        source="python_async_runner",
                        route=f"python.async_runner.{task.task_id}",
                        evidence={"result_type": type(result).__name__},
                    )
                )
            return result

        if not isinstance(yielded, PendingCall):
            invocation.iterator.close()
            self._remove_invocation(task.task_id)
            raise RunnerInvokeError(
                RuntimeError(
                    code="runner.awaitable_unsupported",
                    source="python_async_runner",
                    route=f"python.async_runner.{task.task_id}",
                    evidence={"yielded_type": type(yielded).__name__},
                )
            )
        invocation.pending = yielded
        return _waiting_result(task.task_id, yielded)

    async def run_batch(self, ctx: RunnerContext, batch: WorkBatch) -> CompletionBatch:
        return await ScalarBatchAdapter(self).run_batch(ctx, batch)

    async def cancel(self, invocation_id: str) -> None:
        task_id = self._invocation_tasks.get(invocation_id)
        if task_id is not None:
            self._remove_invocation(task_id)

    async def dispose(self) -> None:
        self._invocations.clear()
        self._invocation_tasks.clear()

    def _track_invocation(self, task_id: str, invocation_id: str) -> None:
        if not invocation_id:
            return
        self._invocation_tasks[invocation_id] = task_id

    def _remove_invocation(self, task_id: str) -> None:
        if self._invocations.pop(task_id, None) is None:
            return
        self._invocation_tasks = {
            invocation_id: known_task_id
            for invocation_id, known_task_id in self._invocation_tasks.items()
            if known_task_id != task_id
        }


class _Invocation:
    def __init__(self, iterator: Generator[Any, TaskOutcome | None, RunnerResult]) -> None:
        self.iterator = iterator
        self.pending: PendingCall | None = None


def _waiting_result(task_id: str, pending: PendingCall) -> RunnerResult:
    tasks = () if pending.task is None else (pending.task,)
    return RunnerResult(
        task_id=task_id,
        tasks=tasks,
        task_await=pending.task_await,
        status=RunnerStatus.WAITING,
    )


def _continuation_ref(parent_task_id: str) -> ResourceRef:
    ref_id = f"continuation:{parent_task_id}"
    return ResourceRef(
        ref_id=ref_id,
        resource_id=ResourceId(
            kind_id="continuation",
            slot_id=ref_id,
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
    )
