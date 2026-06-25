from __future__ import annotations

import mutsuki_runtime_python as runtime_python


def test_public_api_no_longer_exports_agent_backend_compatibility_layer() -> None:
    assert hasattr(runtime_python, "PythonRunnerHost")
    assert hasattr(runtime_python, "RunnerDescriptor")
    assert hasattr(runtime_python, "RunnerInvokeError")
    assert not hasattr(runtime_python, "StrategyBackend")
    assert not hasattr(runtime_python, "OperationBackend")
    assert not hasattr(runtime_python, "PythonBackendHost")
    assert not hasattr(runtime_python, "BackendInvokeError")
