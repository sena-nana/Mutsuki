# Bot Flow Router

`mutsuki.bot.router.flow` executes the single immutable Bot DAG revision applied through
`ConfigService`. The active Flow provider snapshot is the only routing source of truth; plugin
manifests contribute only `mutsuki.bot.flow.nodes@1` catalogs containing node types, typed ports,
configuration schemas and exact `HandlerBinding` targets.

Ingress compares an event with every Source selector on the live graph. QQ Source nodes are split
by received kind (`收到消息`, `成员加入`, `添加表情`, …). Bot self-sent messages
(`ext.bot.self_sent`) do not start any Source chain. Each Source only starts a chain for that
kind and exposes a typed output port so later nodes can require a message, member, reaction, or
lifecycle payload. Every matched Source chain gets an independent execution identity. Source
output edges, node output edges and error edges are explicit; multiple edges fan out, and multiple
incoming edges invoke the target separately. Version 1 rejects cycles and does not infer join,
priority, propagation or hooks.

Match nodes are a series of single-purpose filters: conversation, user, role, prefix, keyword, link,
mention, empty mention, probability, rate limit and QQ event type. Conversation/user/role/rate-limit/QQ-event ports use the
generic `mutsuki.bot.event` type so member, reaction and lifecycle sources can connect; a typed
`mutsuki.bot.event.*` output assigns to that generic input. Each node emits `matched` or
`unmatched`; authors compose them with edges instead of packing account or protocol identifiers
into one form.

Version 1 is an acyclic DAG of single-event predicates. It does not wait, join, cycle, or keep
session state on the canvas. `mutsuki.bot.interaction.create` Creates a waiter as a sync
side-effect through `mutsuki.bot.interaction/handle@1`; `mutsuki.bot.interaction.match` only gates
the current event. The next ingress rematches.

The first-party QQ AI example (`qq.ai.orchestrated`) sequences source → persona-command →
empty-mention → mention. Mention is not wired from source in parallel with the command matcher.
Source also fans out to `record-icl-listen` so every ingress is recorded (QQ group events are
almost always AT, so listen on mention unmatched never runs). Empty-mention matched Creates a
60s waiter; a bare @ does not submit Agent. Empty-mention unmatched rematches on interaction;
`interaction.matched` joins the same record-icl → attach-icl submit chain. Timeout or no waiter
does not enter Agent. Probability and prefix stay in the catalog; the example graph omits them.
The first-party link example (`qq.link.resolve`) sequences source → `mutsuki.bot.match.link` →
Bilibili / Mihuashi resolve → `qq.send`. It requires the Bilibili `web_cookie` catalog node and
full group receive; AT-only traffic will not see unmentioned mini-program shares.

Plugins can start chains without a chat message: a domain runner submits a `flow/ingress@1`
envelope whose `payload.value` is a plugin-owned typed payload and whose `event_type` is a
`mutsuki.bot.event.*` specialization. The matching Source node is selected by that type alone and
is deliberately not listed in the router's `source_kinds_for_node` kind table (that table filters
`BotEvent` kinds; an unknown node type with a non-`BotEvent` payload passes through its selector).
Bilibili pushes work this way: polling detects a fresh item and submits
`mutsuki.bot.event.bilibili` v1 with a `BilibiliNotification`; the first-party push example
(`bilibili.live.push`) sequences `mutsuki.bot.bilibili.notification` → `mutsuki.bot.bilibili.card`
(cover download + `mutsuki.protocol.image` card render, outputs `mutsuki.bot.message.send`) →
`qq.send`. If the active graph has no matching Source, the business behavior behind that event
stays frozen: the envelope is counted in the registry ingress stats (`accepted_total` /
`dropped_total`, published on the `mutsuki.bot.flow.ingress` health snapshot) and its ingress
task output records `matched_sources: 0`. Recreate the push subgraph after upgrading or before
relying on pushes.

Submit uses mention/interaction matched → record-icl → attach-icl → identifiers →
attach-bound-persona → bind-profile → agent → quote → mention-reply → segment →
`mutsuki.bot.qq.reply.forward_fold` → delivery. Presentation/delivery failures take error edges
to `command.reply` then `qq.send`. Conversation-context, reply and persona runners are registered
by the Agent configured plugin against the shared state-db store, not as independent catalog
factories.

Only runners that publish `mutsuki.bot.flow.nodes@1` appear in the editor. Flow does not scan
every `RunnerDescriptor`. Mihuashi, Bilibili resolve, Bilibili workshop and scheduled delivery opt in through that
extension; internal Gateway frame-mapper runners (they emit `flow/ingress@1`) stay off the canvas. Std workflow/io/db
and Agent loop/tool/session use a different orchestration surface.

Processor/Match/Sink execution uses ordinary Task calls through `TaskAwaitRunnerAdapter`. A node is
invoked only through the binding stored in its catalog descriptor. Failures terminate the current
branch unless the node has an error edge, in which case the router emits a typed structured error
event on that edge. Other branches continue independently.

Every node Task pins `graph_revision`, registry generation, trace and correlation. Saving a new
configuration revision atomically changes new ingress only; an in-flight task keeps its immutable
`Arc` snapshot.

Save is revision-fenced: the editor submits one CAS after validation against the current LoadPlan
catalog, and the applied document is immediately live. `ConfigRepository` owns durable storage and
`BotFlowConfigProvider` owns activation/rollback. LoadPlan only admits the node catalog; stored
graphs restore after Host start and never block boot. An incompatible graph stays empty and stops
routing. Editor save still validates synchronously. An empty graph is valid.
