# Runner Supervisor

The runner supervisor owns external process lifecycle for sidecars and non-Core-linked runners.

Implemented behavior:

- spawn process with sanitized allowlist environment
- inject `MUTSUKI_HOME`, `MUTSUKI_RUNNER_SESSION_TOKEN`, `MUTSUKI_RUNNER_ID`, and `MUTSUKI_PLUGIN_ID`
- drain stdout/stderr
- report process state
- restart and stop by runner id
- automatic restart with backoff after an unexpected exit
- graceful shutdown before service exit

## Session token

`MUTSUKI_RUNNER_SESSION_TOKEN` identifies one process incarnation. The supervisor mints it from the
OS CSPRNG on every spawn, including restarts, so the value a previous incarnation held stops being
valid the moment that process is replaced. It is deliberately unrelated to the control token and
carries no control-plane authority: the control socket accepts the control token only, so a
compromised sidecar cannot use its session token to reload plugins or shut the service down.

## Restart policy

`[runners].restart` and `[runners].max_restart_per_minute` drive `RestartPolicy`:

- Only an *unexpected* exit triggers a restart. A `stop` or `shutdown` command is an operator
  decision and is never undone by the policy, including when it arrives during a backoff wait.
- `max_restart_per_minute` is a sliding 60s window, not a lifetime total. A Runner that fails
  rarely keeps recovering; one that crash-loops exhausts the window and is parked in `Failed`
  with `restart budget exhausted` in `last_error` so an operator can see why it stopped.
- Backoff ramps from 100ms, doubling to a 10s ceiling, so a fast crash loop cannot burn the whole
  window in a few milliseconds. A process that stayed up past the ceiling resets the ramp.
- `restart = false` or `max_restart_per_minute = 0` disables restarts. `RestartPolicy::default()`
  is disabled, so an embedder that constructs specs directly does not silently inherit a loop.
- An explicit `restart` command or a `reconcile` pass still starts a Runner that the budget
  parked; the budget bounds automatic recovery, not operator action.

Core-connected `binary-stdio` runners are spawned by ServiceHost and registered with Core through
`mutsuki-runtime-host::BinaryRunner`. Task execution calls `runner.run_batch`, `runner.cancel`, and
`runner.dispose` over binary stdio (`{ ctx, batch }` -> `CompletionBatch`). ServiceHost does not
implement the obsolete `Runner::step` / `runner.step` path.

## Cancellation and isolation

In-process native runners support cooperative cancellation only. A wall-clock deadline can cancel
the Core task and quarantine the worker, but Rust cannot safely terminate the executing thread.
The Host therefore does not create a replacement until that worker actually exits. Once the
configured isolated-worker limit is reached, the pool reports degraded health and refuses further
dispatch instead of accumulating zombie threads.

Process and Python/Script deployments use a process boundary for hard isolation. On hard timeout,
the Host kills the child process through a thread-safe termination handle, waits for the blocked
binary call to return, recreates the process, and only then restores the runner and worker capacity.
Untrusted code, crash isolation, and strict wall-clock termination must use a process/ABI sidecar;
declaring a native runner as `Blocking` or `Script` does not grant thread-level hard termination.
