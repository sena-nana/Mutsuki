from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass, field
from typing import Self

from mutsuki_runner_kit.contracts.codec import (
    JsonDict,
    ScalarValue,
    as_mapping,
    as_scalar_dict,
    as_str,
    field_value,
)

# Mirror of the closed error-code set owned by `mutsuki-runtime-contracts`. Core is the source
# of truth; `tests/test_contracts_error_codes.py` fails if the two sets diverge.
ERR_PLUGIN_DISABLED = "plugin.disabled"
ERR_PLUGIN_NOT_FOUND = "plugin.not_found"
ERR_RUNTIME_ABORTED = "runtime.aborted"
ERR_RUNTIME_HOST_FAILED = "runtime.host_failed"
ERR_RUNTIME_HOST_GENERATION_MISMATCH = "runtime.host_generation_mismatch"
ERR_RUNTIME_NOT_ACCEPTING = "runtime.not_accepting"
ERR_CAPABILITY_EXHAUSTED = "capability.exhausted"
ERR_TASK_NOT_FOUND = "task.not_found"
ERR_TASK_DUPLICATE = "task.duplicate"
ERR_TASK_CLAIM_CONFLICT = "task.claim_conflict"
ERR_TASK_EXPIRED = "task.expired"
ERR_TASK_DEAD_LETTER = "task.dead_letter"
ERR_TASK_UNSUPPORTED = "task.unsupported"
ERR_TASK_CANCEL_POLICY_UNSUPPORTED = "task.cancel_policy_unsupported"
ERR_TASK_SELF_CALL_BLOCKED = "task.self_call_blocked"
ERR_RUNNER_NOT_FOUND = "runner.not_found"
ERR_RUNNER_PURITY_VIOLATION = "runner.purity_violation"
ERR_RUNNER_AWAITABLE_UNSUPPORTED = "runner.awaitable_unsupported"
ERR_REGISTRY_FROZEN = "registry.frozen"
ERR_REGISTRY_UNAUTHORIZED = "registry.unauthorized"
ERR_REGISTRY_GENERATION_MISMATCH = "registry.generation_mismatch"
ERR_RESOURCE_NOT_FOUND = "resource.not_found"
ERR_RESOURCE_GENERATION_MISMATCH = "resource.generation_mismatch"
ERR_RESOURCE_LEASE_EXPIRED = "resource.lease_expired"
ERR_RESOURCE_LEASE_CROSS_AWAIT = "resource.lease_cross_await"
ERR_RESOURCE_UNSUPPORTED = "resource.unsupported"
ERR_STATE_CONFLICT = "state.conflict"
ERR_STATE_HISTORY_DISABLED = "state.history_disabled"
ERR_STATE_HISTORY_UNAVAILABLE = "state.history_unavailable"
ERR_RELOAD_BLOCKED = "plugin.reload_blocked"
ERR_CHECKPOINT_INCOMPATIBLE = "checkpoint.incompatible"
ERR_EXECUTION_NO_VARIANT = "execution.no_variant"
ERR_PORTABLE_SCHEMA_UNSUPPORTED = "portable.schema_unsupported"


@dataclass(frozen=True)
class RuntimeError:
    code: str
    source: str
    route: str
    lost_capability: str | None = None
    recovery: str | None = None
    cause: RuntimeError | None = None
    evidence: dict[str, ScalarValue] = field(default_factory=dict)

    @classmethod
    def from_json_dict(cls, data: Mapping[str, object] | JsonDict) -> Self:
        raw = as_mapping(data, "RuntimeError")
        cause = field_value(raw, "cause")
        lost_capability = field_value(raw, "lost_capability")
        recovery = field_value(raw, "recovery")
        return cls(
            code=as_str(field_value(raw, "code"), "code"),
            source=as_str(field_value(raw, "source"), "source"),
            route=as_str(field_value(raw, "route"), "route"),
            lost_capability=None
            if lost_capability is None
            else as_str(lost_capability, "lost_capability"),
            recovery=None if recovery is None else as_str(recovery, "recovery"),
            cause=None
            if cause is None
            else RuntimeError.from_json_dict(as_mapping(cause, "cause")),
            evidence=as_scalar_dict(field_value(raw, "evidence"), "evidence"),
        )
