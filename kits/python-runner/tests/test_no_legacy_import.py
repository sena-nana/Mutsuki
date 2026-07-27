from __future__ import annotations

import importlib

import pytest

import mutsuki_runner_kit as runtime_python
import mutsuki_runner_kit.contracts as contracts


def test_top_level_facades_do_not_export_runtime_symbols() -> None:
    assert not hasattr(runtime_python, "PythonRunnerBackend")
    assert not hasattr(runtime_python, "RunnerDescriptor")
    assert not hasattr(runtime_python, "RunnerInvokeError")
    assert not hasattr(contracts, "Task")
    assert not hasattr(contracts, "to_json_dict")


def test_removed_flat_modules_are_not_importable() -> None:
    base = "mutsuki_runner_kit"
    for module_name in (f"{base}.host", f"{base}.runner", f"{base}.resource", f"{base}.stdio"):
        with pytest.raises(ModuleNotFoundError):
            importlib.import_module(module_name)


def test_pre_split_package_name_is_not_importable() -> None:
    with pytest.raises(ModuleNotFoundError):
        importlib.import_module("mutsuki_runtime_python")
