"""已删除公共入口的回归保护。"""

from __future__ import annotations

import importlib.util


def test_removed_adapter_namespace_is_not_importable() -> None:
    removed_name = ".".join(("mutsuki", "adapters"))
    assert importlib.util.find_spec(removed_name) is None


def test_removed_runtime_loop_module_is_not_importable() -> None:
    removed_name = ".".join(("mutsuki", "runtime", "loop"))
    assert importlib.util.find_spec(removed_name) is None
