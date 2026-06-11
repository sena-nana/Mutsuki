"""跨 Agent 冒烟入口可运行。"""

from __future__ import annotations

import pytest

from mutsukicore.plugins.cross_agent_smoke import main


@pytest.mark.asyncio
async def test_cross_agent_smoke_runs() -> None:
    await main()
