---
name: plugins
description: Implement or change AgentLoop, context, session, tool router, memory router, model gateway, prompt, model provider integration, or native Agent Runner and bundle behavior.
---

# Agent Plugins

- Put deterministic behavior behind typed service APIs and expose real runtime Runner/bundle entrypoints.
- Route plugin interaction through declared Agent protocols; direct composition is only for local deterministic tests or embedding.
- Keep runners batch-first and isolate each entry failure.
- Obtain Provider credentials through Host secret injection and keep clients inside gateways.
- Reject effectful providers from inline gateway calls; execute network providers only in the declared effect Runner.
- Emit structured results/events and declare matching manifest capabilities.

Do not move Core scheduling, Host lifecycle or Bot platform translation into Agent plugins.
