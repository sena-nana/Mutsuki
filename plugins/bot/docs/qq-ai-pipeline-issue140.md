# QQ AI pipeline functional boundary (Issue #140)

The QQ AI pipeline keeps ownership split by public protocols:

```text
QQ Source -> Event/Command Match -> Agent Processor -> reliable Delivery Sink
          \-> other explicit processor/sink branches
```

`QqConversationRef` is the stable private/group/channel identity. Its v1 origin key is length
delimited and can be parsed back during state migration; persisted keys are validated before they
are used to rebuild a target. The active Bot Flow configuration owns mention/wake-word, allow/deny,
account/role/rate-limit and command matching. Agent conversation state keeps only profile,
session scope, STT/TTS and delivery execution settings. Session bindings and processed event
claims are durable and generation-fenced. Reset and expiry create a new binding while preserving the old
generation fence; fork invokes Agent `ForkSession` and only then commits the new binding with
compare-and-set.

Command words are graph data rather than plugin declarations. Agent exposes submit, cancel,
reset, fork, status and regenerate Processor nodes. Timeout and maximum concurrency are installed as ServiceHost runner limits,
so cancellation and capacity enforcement remain runtime-owned. Config Web applies those limits
through the same generation reload transaction; a successful reload replaces the scheduler values
rather than only updating the persisted form.

If a public Agent submit has already been accepted but event retrieval disconnects, the persisted
pending event claim and session version are resumed after bridge reload. The idempotency key and
last event sequence prevent a second SubmitTurn while allowing the missing event page to be read.

Bot Agent runtime settings are owned by `mutsuki-plugin-bot-agent` as the versioned
`BotAgentConfig` provider `mutsuki.plugin.bot.agent`: enablement, default Agent profile, reply
mode, concurrency, timeout, and QQ text boundary. The explicit `QqAiBotPluginBundle` publishes a
shared config host service. The embedded Config Web backend registers the provider only when that
live service and a product selection are both present; schema, revision checks, validation,
atomic product-file persistence, and plugin-reload lifecycle are then real control-plane paths.
Without a live bridge, a selected Bot Agent configuration fails startup instead of rendering an
unbound settings page. Session/profile execution settings remain in the Conversation owner; all
permission and trigger policy remains in Flow Match nodes. The bridge only injects the opaque
profile ID into the public AgentKit session request. The product Host compiles its approved
instructions and profile overrides into that `AgentRuntimeProfile`, so BotPlugins do not recreate a
Persona or depend on AgentKit Runtime internals.

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

Functional acceptance consists of owner crate tests plus the ServiceRuntime E2E that applies a
graph, routes QQ ingress through Agent and reliable delivery, suppresses replay, and recovers the
same graph after restart. Flow Web RPC tests cover authenticated permission, browser-local draft,
validation and apply CAS conflict. QQ Web keeps login and live connection status. Flow editing remains a separate real
WebExtension with its own revision and permission contracts.
It cannot be registered without an injected management API and is not enabled by the default
catalog, so a product must bind those actions to its live account/secret/audit provider before the
page exists; the monorepo does not install a fake production provider.
