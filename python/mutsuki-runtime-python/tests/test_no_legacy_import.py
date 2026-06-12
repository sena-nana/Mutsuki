from __future__ import annotations

import importlib
import sys


def test_new_package_does_not_import_reference_mutsuki_core() -> None:
    for name in tuple(sys.modules):
        if name == "mutsuki" or name.startswith("mutsuki."):
            sys.modules.pop(name)
        if name == "mutsuki_ext" or name.startswith("mutsuki_ext."):
            sys.modules.pop(name)

    importlib.import_module("mutsuki_runtime_python")

    assert "mutsuki" not in sys.modules
    assert "mutsuki_ext" not in sys.modules
