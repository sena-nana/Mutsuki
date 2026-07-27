# Migration

AgentKit does not include `packages/python/` or Python Agent decorators.

If Python tools are needed, expose them as normal Mutsuki protocols through a Python runner plugin and register Agent tool metadata from Rust or from a separate optional Python Agent SDK repository.
