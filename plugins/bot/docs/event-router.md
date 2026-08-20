# Bot Flow Router

`mutsuki.bot.router.flow` executes the single immutable Bot DAG revision applied through
`ConfigService`. The active Flow provider snapshot is the only routing source of truth; plugin
manifests contribute only `mutsuki.bot.flow.nodes@1` catalogs containing node types, typed ports,
configuration schemas and exact `HandlerBinding` targets.

Ingress compares an event with every Source selector on the live graph. QQ Source nodes are split
by received kind (`收到消息`, `成员加入`, `添加表情`, …). Each Source only starts a chain for that
kind and exposes a typed output port so later nodes can require a message, member, reaction, or
lifecycle payload. Every matched Source chain gets an independent execution identity. Source
output edges, node output edges and error edges are explicit; multiple edges fan out, and multiple
incoming edges invoke the target separately. Version 1 rejects cycles and does not infer join,
priority, propagation or hooks.

Match nodes are a series of single-purpose filters: conversation, user, role, prefix, keyword,
mention and rate limit. Each node emits `matched` or `unmatched`; authors compose them with edges
instead of packing account or protocol identifiers into one form.

Processor/Match/Sink execution uses ordinary Task calls through `TaskAwaitRunnerAdapter`. A node is
invoked only through the binding stored in its catalog descriptor. Failures terminate the current
branch unless the node has an error edge, in which case the router emits a typed structured error
event on that edge. Other branches continue independently.

Every node Task pins `graph_revision`, registry generation, trace and correlation. Saving a new
configuration revision atomically changes new ingress only; an in-flight task keeps its immutable
`Arc` snapshot.

Save is revision-fenced: the editor submits one CAS after validation against the current LoadPlan
catalog, and the applied document is immediately live. `ConfigRepository` owns durable storage and
`BotFlowConfigProvider` owns activation/rollback. Cold boot and plugin reload run the same catalog
validation through a domain-neutral ServiceHost LoadPlan hook. A missing node, port or binding
prevents activation and leaves the previous graph active. An empty graph is valid and stops routing.
