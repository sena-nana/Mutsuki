# Bot Flow Router

`mutsuki.bot.router.flow` executes immutable Bot DAG revisions published by the Web Console. The
graph stored by the Bot owner is the only routing source of truth; plugin manifests contribute only
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

Every node Task pins `graph_revision`, registry generation, trace and correlation. Publishing a new
revision atomically changes new ingress only; an in-flight node reloads its immutable pinned graph
version from the Bot repository.

Publishing is revision-fenced: validate against the current LoadPlan catalog, durably persist the
new version and audit record, then atomically activate it. Cold boot and plugin reload run the same
catalog validation through a domain-neutral ServiceHost LoadPlan hook. A missing node, port or
binding prevents activation and leaves the previous graph active.
