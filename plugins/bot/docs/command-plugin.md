# Command Match Node

The Command plugin contributes the `mutsuki.bot.command.match@1` Match node. It does not own a
global prefix or command directory and does not generate business bindings from command names.

Each node instance stores its own prefixes, command path, aliases, typed arguments and
case-sensitivity in the published graph. The editor shows these as command fields, not protocol or
node identifiers. It consumes `mutsuki.bot.event@1` and emits exactly one of:

- `matched`: a typed `mutsuki.bot.command.event@1` envelope;
- `unmatched`: the unchanged input event.

The Web editor derives the property panel from the node configuration schema. Business plugins
declare behavior nodes accepting the command event; the graph edge determines which behavior is
called. Invalid typed arguments fail that node branch with a structured parse error, while a missing
prefix or another command path uses the explicit `unmatched` output.

Builtin and ABI v2 deployments publish the same node catalog and binding business surface. Legacy
`prefixes`/`commands` plugin configuration is rejected because the configured factory accepts only
an empty owner config.
