from __future__ import annotations

import json
from importlib.resources import files
from typing import cast

from mutsuki_runner_kit.contracts.codec import JsonDict


def load_runtime_wire_schema() -> JsonDict:
    path = files("mutsuki_runner_kit.wire").joinpath("runtime-wire-v1.json")
    loaded = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(loaded, dict):
        raise TypeError("runtime wire schema must be an object")
    return cast(JsonDict, loaded)


RUNTIME_WIRE_SCHEMA = load_runtime_wire_schema()

