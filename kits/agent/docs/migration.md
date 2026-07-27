# AgentKit migration

## From `mutsuki-agent-protocol`

The former catch-all protocol crate has been replaced by `mutsuki-agent-contracts`. Consumers must
depend on the owner package directly. Runtime behavior belongs to `mutsuki-agent-runtime`, model
protocol integration to `mutsuki-agent-adapter-*`, and extension surfaces to
`mutsuki-agent-plugin-*`.

## From direct provider/tool calls

Model calls must be registered as Adapter `AsyncBatchHandler`s. Tool calls must expose a normal
Mutsuki protocol runner and register `AgentToolDescriptor` metadata. The Agent loop submits both
through `RuntimeClient`; direct callbacks into private Runtime objects are unsupported.

## From product-owned agent cores

Products should compile settings and optional Persona data into `AgentRuntimeProfile`, then assemble
the public bundle in their Host layer. Move session/turn/approval/budget semantics to Agent Runtime,
but keep workspace context, UI, Persona, secrets and product commands in the product repository.
No `LiliaCodeCore` compatibility shim is required.

## Session persistence

In-memory `SessionStore` remains suitable for ephemeral runs. Durable products configure
`AgentDatabaseSessionStore` with a `mutsuki-protocol-db` executor and persist
`AgentSessionCheckpoint`. Resume must use a new coordinator epoch and validate checkpoint/profile
identity before side effects are re-enabled.

## Client and remote execution

Replace private RPC DTOs with `mutsuki-agent-client` and the versioned Agent wire envelope over
MutsukiLink. Map remote adapter or sub-agent work through
`mutsuki-agent-distributed-integration`; approval-bound and non-idempotent work remains local unless
the profile explicitly supplies a valid placement policy.

## Python

AgentKit does not include Python Agent decorators. Python tools are normal Mutsuki protocols exposed
by Python Runner Kit; Rust or a separate optional SDK registers their Agent tool metadata.

## Compatibility checks

Pin one monorepo tag/commit and its root `Cargo.lock`; do not mix old standalone AgentKit, Core,
Link, Host, Database or Distributed revisions. Re-run contracts, plugin conformance, Host matrix and
the deterministic performance model after migration.
