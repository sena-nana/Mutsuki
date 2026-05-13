"""DAG 拓扑排序 + 环检测。"""

from __future__ import annotations

import pytest

from nanobot.core.loader import PluginCycleError, _toposort


def test_simple_chain_orders_dependency_first() -> None:
    items: dict[str, tuple[str, ...]] = {"a": ("b",), "b": ("c",), "c": ()}
    order = _toposort(items)
    assert order.index("c") < order.index("b") < order.index("a")


def test_cycle_raises_plugin_cycle_error() -> None:
    items: dict[str, tuple[str, ...]] = {"a": ("b",), "b": ("a",)}
    with pytest.raises(PluginCycleError):
        _toposort(items)


def test_independent_nodes_all_emitted() -> None:
    items: dict[str, tuple[str, ...]] = {"a": (), "b": (), "c": ()}
    order = _toposort(items)
    assert sorted(order) == ["a", "b", "c"]
