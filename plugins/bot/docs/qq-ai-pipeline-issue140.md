# QQ AI pipeline functional boundary (Issue #140)

The QQ AI pipeline keeps ownership split by public protocols:

```text
QQ adapter -> event ingest -> interaction waiter -> handler pipeline
           -> command / media bridge -> Agent bridge -> message send
Scheduled Agent result -> opaque conversation binding -> active delivery
```

`QqConversationRef` is the stable private/group/channel identity. Its v1 origin key is length
delimited and can be parsed back during state migration; persisted keys are validated before they
are used to rebuild a target. Conversation policy resolves mention/wake-word, allow/deny, Agent
profile, session scope, STT/TTS, and active-delivery admission. Matching rules are applied in the
fixed order product default → account → group/guild → channel → conversation → actor, and the
resolved result reports the rule source chain. Session bindings and processed event claims are
durable and generation-fenced. Reset and expiry create a new binding while preserving the old
generation fence; fork invokes Agent `ForkSession` and only then commits the new binding with
compare-and-set.

The built-in commands are `/ask`, `/chat`, `/cancel <turn_id>`, `/reset`, `/fork`, `/status`, and
`/regenerate`. Handler timeout and maximum concurrency are installed as ServiceHost runner limits,
so cancellation and capacity enforcement remain runtime-owned.

QQ capability reporting is authoritative per account. The adapter currently advertises final-only
and segmented-message streaming. It does not advertise message editing, so the pipeline does not
offer a fake message-update strategy. Inbound and outbound image/audio/file support is admitted
only for validated `ResourceRef` values; STT and TTS cross the public Agent media protocols.

Active delivery persists request, attempts, per-part receipts, retry schedule, and idempotency key.
Preview, inspection, manual retry, and cancellation are implemented by the Delivery owner service
and exposed as typed management actions.
Scheduled Agent delivery accepts only a successful run with an opaque persisted Bot conversation
binding; raw QQ identifiers in schedule metadata are never trusted.
The optional scheduled-delivery protocol runner connects a public `ScheduledRunResult` callback to
that bridge inside ServiceRuntime; it is installed only when a target resolver and policy provider
are explicitly supplied.

Multi-step interaction state is durable and compare-and-set fenced. Predicate rejection consumes a
retry, emits the configured retry prompt through `BOT_MESSAGE_SEND_PROTOCOL_ID`, and either remains
waiting or becomes failed. Completed steps can transition to a new state/wait specification; reload
recovery cancels stale generations and deterministically times out expired waiters.

Functional acceptance consists of owner crate tests plus the ServiceRuntime E2E that routes a QQ
event with audio through STT, Agent, TTS, QQ text/voice delivery, replay suppression, and `/ask`
command dispatch. The same E2E covers a two-step non-Agent interaction and an idempotent scheduled
Agent result delivery. The QQ Web extension exposes account, capability, policy, command, session,
delivery, interaction, revision, permission, confirmation, and audit-backed management contracts.
It cannot be registered without an injected management API and is not enabled by the default
catalog, so a product must bind those actions to its live account/secret/audit provider before the
page exists; the monorepo does not install a fake production provider.
