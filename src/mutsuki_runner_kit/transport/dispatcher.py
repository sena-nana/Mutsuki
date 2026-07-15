from __future__ import annotations

import asyncio
from typing import Protocol, assert_never

from mutsuki_runner_kit.contracts.codec import JsonValue, to_json_dict
from mutsuki_runner_kit.contracts.resource import (
    CommandBatch,
    CommandPlan,
    ExportPlan,
    PlanReceipt,
    SagaPlan,
)
from mutsuki_runner_kit.runners.backend import PythonRunnerBackend
from mutsuki_runner_kit.wire.protocol import ProtocolHelloAck, WireProtocolFailure
from mutsuki_runner_kit.wire.requests import (
    CancelRunnerRequest,
    CommandBatchRequest,
    CommandPlanRequest,
    DisposeRunnerRequest,
    ExportPlanRequest,
    InitializeRequest,
    RunBatchRequest,
    RunnerWireRequest,
    SagaPlanRequest,
)


class ResourceRequestHandler(Protocol):
    def execute_export_plan(self, plan: ExportPlan) -> PlanReceipt: ...

    def execute_command_plan(self, plan: CommandPlan) -> PlanReceipt: ...

    def execute_command_batch(self, batch: CommandBatch) -> tuple[PlanReceipt, ...]: ...

    def execute_saga_plan(self, saga: SagaPlan) -> tuple[PlanReceipt, ...]: ...


class RunnerRequestDispatcher:
    def __init__(
        self,
        runner_backend: PythonRunnerBackend,
        resource_handler: ResourceRequestHandler | None,
        codec_id: str,
    ) -> None:
        self._runner_backend = runner_backend
        self._resource_handler = resource_handler
        self._codec_id = codec_id
        self._negotiated = False
        self._negotiation_lock = asyncio.Lock()

    async def dispatch(self, request: RunnerWireRequest) -> JsonValue:
        if isinstance(request, InitializeRequest):
            async with self._negotiation_lock:
                ack = ProtocolHelloAck.negotiate(request.hello, self._codec_id)
                self._negotiated = True
                return _json_value(ack.to_dict())
        if not self._negotiated:
            raise WireProtocolFailure(
                "wire.not_initialized",
                "plugin.initialize must succeed before business requests",
            )
        if isinstance(request, RunBatchRequest):
            completion = await self._runner_backend.run_batch_runner(
                request.runner_id, request.ctx, request.batch
            )
            return to_json_dict(completion)
        if isinstance(request, CancelRunnerRequest):
            await self._runner_backend.cancel_runner(
                request.runner_id, request.invocation_id
            )
            return None
        if isinstance(request, DisposeRunnerRequest):
            await self._runner_backend.dispose_runner(request.runner_id)
            return None
        resources = self._resource_handler
        if resources is None:
            raise WireProtocolFailure(
                "wire.resource_handler_missing",
                "resource request handler was not injected",
            )
        if isinstance(request, ExportPlanRequest):
            return to_json_dict(
                await asyncio.to_thread(resources.execute_export_plan, request.plan)
            )
        if isinstance(request, CommandPlanRequest):
            return to_json_dict(
                await asyncio.to_thread(resources.execute_command_plan, request.plan)
            )
        if isinstance(request, CommandBatchRequest):
            receipts = await asyncio.to_thread(
                resources.execute_command_batch, request.batch
            )
            return [to_json_dict(receipt) for receipt in receipts]
        if isinstance(request, SagaPlanRequest):
            receipts = await asyncio.to_thread(resources.execute_saga_plan, request.saga)
            return [to_json_dict(receipt) for receipt in receipts]
        assert_never(request)


def _json_value(value: object) -> JsonValue:
    from mutsuki_runner_kit.contracts.codec import as_json_value

    return as_json_value(value)
