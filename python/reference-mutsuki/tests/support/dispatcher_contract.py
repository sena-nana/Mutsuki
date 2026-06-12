from __future__ import annotations

from collections.abc import Iterable

from mutsuki.core.agent import Agent
from mutsuki.core.loader import PluginLoader


async def assert_dispatcher_clean_after_unload(
    loader: PluginLoader,
    agent: Agent,
    *,
    operations: Iterable[str] = (),
    sources: Iterable[str] = (),
) -> None:
    """Unload plugins and assert dispatcher Operation/Source tables are clean."""
    await loader.unload_from(agent)
    for op_id in operations:
        assert not agent.dispatch.has_operation(op_id), op_id
    for source_id in sources:
        assert not agent.dispatch.has_source(source_id), source_id

