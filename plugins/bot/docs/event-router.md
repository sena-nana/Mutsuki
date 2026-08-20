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

Match nodes are a series of single-purpose filters: conversation, user, role, prefix, keyword,
mention, rate limit and QQ event type. Conversation/user/role/rate-limit/QQ-event ports use the
generic `mutsuki.bot.event` type so member, reaction and lifecycle sources can connect; a typed
`mutsuki.bot.event.*` output assigns to that generic input. Each node emits `matched` or
`unmatched`; authors compose them with edges instead of packing account or protocol identifiers
into one form.

Version 1 is an acyclic DAG of single-event predicates. It does not wait for a later message,
time out a branch, retry in a loop, or keep session state on the canvas. Interaction sessions
(`Waiting` / `Completed` / `TimedOut` / `Cancelled`) live in `mutsuki.bot.interaction.match`
outside the graph; that node only gates the current event. Wait, join and cycle nodes are out of
scope for V1.

Only runners that publish `mutsuki.bot.flow.nodes@1` appear in the editor. Flow does not scan
every `RunnerDescriptor`. Mihuashi, Bilibili workshop and scheduled delivery opt in through that
extension; internal ingest runners such as the QQ gateway stay off the canvas. Std workflow/io/db
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
