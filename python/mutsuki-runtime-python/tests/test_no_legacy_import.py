from __future__ import annotations

import importlib
import sys


def test_new_package_does_not_import_reference_mutsukibot_core() -> None:
    for name in tuple(sys.modules):
        if name == "mutsukibot" or name.startswith("mutsukibot."):
            sys.modules.pop(name)
        if name == "mutsukibot_ext" or name.startswith("mutsukibot_ext."):
            sys.modules.pop(name)

    importlib.import_module("mutsuki_runtime_python")

    assert "mutsukibot" not in sys.modules
    assert "mutsukibot_ext" not in sys.modules
