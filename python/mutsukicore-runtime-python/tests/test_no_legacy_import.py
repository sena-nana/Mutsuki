from __future__ import annotations

import importlib
import sys


def test_new_package_does_not_import_reference_mutsukicore_core() -> None:
    for name in tuple(sys.modules):
        if name == "mutsukicore" or name.startswith("mutsukicore."):
            sys.modules.pop(name)
        if name == "mutsukicore_ext" or name.startswith("mutsukicore_ext."):
            sys.modules.pop(name)

    importlib.import_module("mutsukicore_runtime_python")

    assert "mutsukicore" not in sys.modules
    assert "mutsukicore_ext" not in sys.modules
