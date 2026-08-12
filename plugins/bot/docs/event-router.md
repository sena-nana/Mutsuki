# Bot Flow Router

`mutsuki.bot.router.flow` executes immutable Bot DAG revisions applied through `ConfigService`. The
active Flow provider snapshot is the only routing source of truth; plugin manifests contribute only
`mutsuki.bot.flow.nodes@1` catalogs containing node types, typed ports, configuration schemas and
exact `HandlerBinding` targets.

Ingress compares an event with every enabled Source selector in the active snapshot. Every matched
flow gets an independent execution identity. Source output edges, node output edges and error edges
are explicit; multiple edges fan out, and multiple incoming edges invoke the target separately.
Version 1 rejects cycles and does not infer join, priority, propagation or hooks.

Processor/Match/Sink execution uses ordinary Task calls through `TaskAwaitRunnerAdapter`. A node is
invoked only through the binding stored in its catalog descriptor. Failures terminate the current
branch unless the node has an error edge, in which case the router emits a typed structured error
event on that edge. Other branches and flows continue independently.

Every node Task pins `graph_revision`, registry generation, trace and correlation. Applying a new
configuration revision atomically changes new ingress only; an in-flight task keeps its immutable
`Arc` snapshot.

Apply is revision-fenced: the browser keeps its draft and base revision, then submits one CAS after
validation against the current LoadPlan catalog. `ConfigRepository` owns durable storage and
`BotFlowConfigProvider` owns activation/rollback. Cold boot and plugin reload run the same catalog
validation through a domain-neutral ServiceHost LoadPlan hook. A missing node, port or binding
prevents activation and leaves the previous graph active.
