from __future__ import annotations

import pytest

from mutsuki_runtime_python.contracts.codec import from_json_dict, to_json_dict
from mutsuki_runtime_python.contracts.plugin import PluginProvides, RuntimeLoadPlan
from mutsuki_runtime_python.contracts.resource import ResourceAccess, ResourceRef
from mutsuki_runtime_python.contracts.runner import RunnerDescriptor
from mutsuki_runtime_python.contracts.surface import SurfaceOccupancyHandle
from mutsuki_runtime_python.contracts.task import Task


def test_missing_required_contract_fields_fail() -> None:
    with pytest.raises(TypeError):
        from_json_dict(Task, {"task_id": "task-1", "protocol_id": "raw.input"})
    with pytest.raises(TypeError):
        from_json_dict(RunnerDescriptor, {"runner_id": "runner-a"})
    with pytest.raises(TypeError):
        from_json_dict(RuntimeLoadPlan, {"lock_version": 1})
    with pytest.raises(TypeError):
        from_json_dict(
            PluginProvides,
            {
                "runners": [],
                "protocols": [],
                "handler_bindings": [],
                "resource_schemas": [],
                "resource_providers": [],
                "effects": [],
            },
        )
    with pytest.raises(TypeError):
        from_json_dict(SurfaceOccupancyHandle, {"handle_id": "timer:1"})
    with pytest.raises(TypeError):
        from_json_dict(ResourceRef, {"ref_id": "resource:1"})
    with pytest.raises(TypeError):
        from_json_dict(ResourceAccess, {"type": "mmap_file", "path": "resource.bin"})


def test_to_json_dict_rejects_non_object_top_level() -> None:
    with pytest.raises(TypeError):
        to_json_dict("not-an-object")
