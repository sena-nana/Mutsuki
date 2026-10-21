---
name: control-api
description: Change ServiceHost local control requests, responses, IPC transports, authentication, status, health, task operations, plugin control, runner control, log tailing, or CLI-facing contracts.
---

# Control API

- Keep the control plane local by default and token-authenticated; debug TCP requires explicit enablement.
- `mutsuki-service-control` (`ControlCommand` / `ControlResult` / `ControlMethod`) is the contract; there is no
  separate hand-written schema. Add a method there once: the Web console exposes every
  `ControlMethod::ALL` entry automatically and derives `runtime.read`/`runtime.write` from `is_mutating`.
- Expose service/core/plugin/runner/task/log/health control only, not a parallel business runtime.
- Use the public `ControlClient` with endpoint, transport and token; never require consumers to load `ServiceConfig`.
  Server-side code that already owns a `ServiceConfig` builds client settings with
  `mutsuki_service_runtime::local_control_client_config`; the IPC client crate has no config conversion.
- Handlers never park an async worker: bounded waits (`TaskWait`, capped at
  `MAX_TASK_WAIT_TIMEOUT_MS`) wait on the blocking pool, and reload drains run via `block_in_place`.
- Test-only bypasses (empty tokens, fixture handlers) must not exist in production builds; fixtures sit
  behind an explicit `testkit` feature enabled only from dev-dependencies.
- Reject arbitrary plugin operations; domain calls must enter Core as tasks.
- Use `TaskHandle` for cancellation and outcomes and preserve structured errors.
- Return real backend state; missing capability must be unavailable, never fabricated.
- Keep secrets out of responses and logs.

Test authentication, transport boundaries, invalid requests and real state transitions.
