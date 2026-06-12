"""Capability 准入检查。"""

from __future__ import annotations

import pytest

from mutsuki.contracts.capability_builtin import Caps
from mutsuki.core.capability_guard import (
    CapabilityNotDeclaredError,
    check_capabilities,
)


def test_required_subset_of_declared_passes() -> None:
    check_capabilities(
        plugin_id="p",
        declared=(Caps.READ_MESSAGE, Caps.SEND_MESSAGE),
        required=(Caps.READ_MESSAGE,),
        route="cmd.x",
    )


def test_missing_capability_raises() -> None:
    with pytest.raises(CapabilityNotDeclaredError) as exc:
        check_capabilities(
            plugin_id="p",
            declared=(Caps.READ_MESSAGE,),
            required=(Caps.SEND_MESSAGE,),
            route="cmd.x",
        )
    assert exc.value.error.code == "capability.not_declared"
