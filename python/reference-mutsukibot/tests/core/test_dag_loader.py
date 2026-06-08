"""DAG 拓扑排序 + 环检测 + 缺失依赖检测。"""

from __future__ import annotations

import pytest

from mutsukibot.contracts.error import Errs
from mutsukibot.core.loader import (
    PluginCycleError,
    PluginDependencyMissingError,
    _toposort,
)


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


def test_missing_dependency_raises_with_structured_error() -> None:
    """A 依赖 B 但 B 不在装载列表里，应当早 fail，而不是延迟到运行时。"""
    items: dict[str, tuple[str, ...]] = {"a": ("b",)}
    with pytest.raises(PluginDependencyMissingError) as ei:
        _toposort(items)
    assert ei.value.missing == [("a", "b")]
    assert ei.value.error.code == Errs.PLUGIN_DEPENDENCY_MISSING
    missing_str = ei.value.error.evidence["missing"]
    assert isinstance(missing_str, str)
    assert "a->b" in missing_str


def test_multiple_missing_dependencies_all_listed() -> None:
    items: dict[str, tuple[str, ...]] = {"a": ("b", "c"), "d": ("e",)}
    with pytest.raises(PluginDependencyMissingError) as ei:
        _toposort(items)
    assert ("a", "b") in ei.value.missing
    assert ("a", "c") in ei.value.missing
    assert ("d", "e") in ei.value.missing
