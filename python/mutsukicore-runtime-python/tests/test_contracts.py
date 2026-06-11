from __future__ import annotations

import pytest

from mutsukicore_runtime_python.contracts import (
    AgentSpec,
    Envelope,
    OperationDescriptor,
    OperationHandlerKey,
    OperationSnapshot,
    OperationStatus,
    PluginAccessState,
    PluginDescriptor,
    PluginSnapshot,
    PluginStatus,
    RuntimeError,
    RuntimeEvent,
    RuntimeEventKind,
    ScopeRuleSpec,
    SourceDescriptor,
    SourceRef,
    SourceSnapshot,
    SpanStatus,
    StrategyResult,
    StrategyResultStatus,
    TraceSpan,
    from_json_dict,
    to_json_dict,
)
from mutsukicore_runtime_python.testing import assert_json_roundtrip


def _envelope() -> Envelope:
    return Envelope(
        id="env-1",
        timestamp=1.0,
        source=SourceRef(
            source_id="source:test",
            kind="test",
            metadata={"stream": "main", "partition": 1, "trusted": True},
        ),
        payload_schema_id="test.input.created",
        capabilities_required=("read", "write"),
        payload={"value": "ok"},
    )


def test_operation_descriptor_matches_rust_wire_shape() -> None:
    descriptor = OperationDescriptor(
        op_id="echo.echo",
        name="echo",
        description="Echo input",
        plugin_id="echo",
        func_qualname="EchoPlugin.echo",
        parameters_schema={"type": "object"},
        return_schema={"type": "string"},
        perms_rule_id="public",
        requires_capabilities=("send_message",),
        is_tool=True,
    )

    assert to_json_dict(descriptor) == {
        "op_id": "echo.echo",
        "name": "echo",
        "description": "Echo input",
        "plugin_id": "echo",
        "func_qualname": "EchoPlugin.echo",
        "parameters_schema": {"type": "object"},
        "return_schema": {"type": "string"},
        "perms_rule_id": "public",
        "requires_capabilities": ["send_message"],
        "is_tool": True,
    }
    assert_json_roundtrip(OperationDescriptor, descriptor)


@pytest.mark.parametrize(
    ("contract_type", "payload"),
    [
        (AgentSpec, {"agent_id": "agent-a"}),
        (
            Envelope,
            {
                "id": "env-1",
                "timestamp": 1.0,
                "source": {"source_id": "source:test", "kind": "test", "metadata": {}},
            },
        ),
        (OperationDescriptor, {"op_id": "plugin.echo", "name": "echo"}),
        (StrategyResult, {"status": "wait_input"}),
        (TraceSpan, {"trace_id": "trace-1", "span_id": "span-1"}),
        (RuntimeEvent, {"sequence": 1, "kind": "trace", "name": "trace.span"}),
        (PluginDescriptor, {"plugin_id": "plugin"}),
        (PluginAccessState, {"enabled_plugin_ids": ["plugin"]}),
    ],
)
def test_contract_decoders_reject_missing_fields(
    contract_type: type[object],
    payload: dict[str, object],
) -> None:
    with pytest.raises(TypeError):
        from_json_dict(contract_type, payload)


@pytest.mark.parametrize(
    ("contract_type", "payload"),
    [
        (ScopeRuleSpec, {"type": "all"}),
        (ScopeRuleSpec, {"type": "by_schema"}),
        (
            RuntimeError,
            {
                "code": "runtime.backend_failed",
                "source": "test",
                "route": "test.route",
                "evidence": {},
            },
        ),
    ],
)
def test_contract_decoders_reject_missing_variant_and_nullable_fields(
    contract_type: type[object],
    payload: dict[str, object],
) -> None:
    with pytest.raises(TypeError):
        from_json_dict(contract_type, payload)


def test_scope_rule_uses_tagged_rust_shape_and_matches_envelopes() -> None:
    envelope = _envelope()
    rule = ScopeRuleSpec.all(
        (
            ScopeRuleSpec.by_schema_prefix("test.input."),
            ScopeRuleSpec.any(
                (
                    ScopeRuleSpec.by_schema("missing"),
                    ScopeRuleSpec.by_source_id("source:test"),
                )
            ),
            ScopeRuleSpec.by_source_kind("test"),
            ScopeRuleSpec.by_capability("write"),
            ScopeRuleSpec.by_source_field("stream", "main"),
        )
    )

    assert rule.matches(envelope)
    assert ScopeRuleSpec.never().matches(envelope) is False
    assert ScopeRuleSpec.always().matches(envelope)
    assert ScopeRuleSpec.by_source_field("partition", 1).matches(envelope)
    assert ScopeRuleSpec.by_capability("missing").matches(envelope) is False
    assert to_json_dict(rule) == {
        "type": "all",
        "parts": [
            {"type": "by_schema_prefix", "prefix": "test.input."},
            {
                "type": "any",
                "parts": [
                    {"type": "by_schema", "schema_id": "missing"},
                    {"type": "by_source_id", "source_id": "source:test"},
                ],
            },
            {"type": "by_source_kind", "kind": "test"},
            {"type": "by_capability", "capability": "write"},
            {"type": "by_source_field", "field": "stream", "value": "main"},
        ],
    }
    assert_json_roundtrip(ScopeRuleSpec, rule)


def test_nested_contract_roundtrips() -> None:
    descriptor = OperationDescriptor(op_id="plugin.echo", name="echo", plugin_id="plugin")
    snapshot = OperationSnapshot(
        descriptor=descriptor,
        status=OperationStatus.ACTIVE,
        key=OperationHandlerKey(
            plugin_id="plugin",
            plugin_generation=0,
            op_id="plugin.echo",
            handler_id="plugin:plugin.echo:0",
        ),
    )
    source = SourceSnapshot(
        descriptor=SourceDescriptor(source_id="source:test", kind="test"),
        plugin_id="plugin",
        plugin_generation=0,
    )
    result = StrategyResult(
        status=StrategyResultStatus.CONTINUE,
        decision={"next": "wait"},
        emitted=(_envelope(),),
    )
    trace = TraceSpan(
        trace_id="trace-1",
        span_id="span-1",
        parent_span_id=None,
        name="agent.input",
        start=1.0,
        end=2.0,
        attributes={"agent_id": "agent-a"},
        status=SpanStatus.OK,
    )

    assert_json_roundtrip(OperationSnapshot, snapshot)
    assert_json_roundtrip(SourceSnapshot, source)
    assert_json_roundtrip(StrategyResult, result)
    assert_json_roundtrip(TraceSpan, trace)


def test_plugin_contract_roundtrips() -> None:
    descriptor = PluginDescriptor(
        plugin_id="plugin",
        generation=7,
        name="Plugin",
        description="Test plugin",
        version="1.0.0",
        capabilities=("source", "operation"),
        metadata={"owned_by": "lilia", "priority": 1},
    )
    snapshot = PluginSnapshot(descriptor=descriptor, status=PluginStatus.ENABLED)
    access = PluginAccessState(
        enabled_plugin_ids=("plugin",),
        disabled_plugin_ids=("disabled-plugin",),
    )

    assert_json_roundtrip(PluginDescriptor, descriptor)
    assert_json_roundtrip(PluginSnapshot, snapshot)
    assert_json_roundtrip(PluginAccessState, access)


def test_runtime_event_matches_rust_wire_shape() -> None:
    event = RuntimeEvent(
        sequence=1,
        kind=RuntimeEventKind.ROUTING,
        name="runtime.publish",
        agent_id="agent-a",
        attributes={"source_id": "source:test"},
        error=RuntimeError(
            code="scope.no_match",
            source="runtime.route",
            route="runtime.publish.source:test",
        ),
    )

    assert to_json_dict(event) == {
        "sequence": 1,
        "kind": "routing",
        "name": "runtime.publish",
        "agent_id": "agent-a",
        "attributes": {"source_id": "source:test"},
        "error": {
            "code": "scope.no_match",
            "source": "runtime.route",
            "route": "runtime.publish.source:test",
            "lost_capability": None,
            "recovery": None,
            "cause": None,
            "evidence": {},
        },
    }
    assert_json_roundtrip(RuntimeEvent, event)


def test_top_level_package_exports_new_runtime_surface() -> None:
    from mutsukicore_runtime_python import (
        ERR_CAPABILITY_EXHAUSTED as exported_error,
    )
    from mutsukicore_runtime_python import (
        RuntimeEvent as ExportedRuntimeEvent,
    )
    from mutsukicore_runtime_python import (
        RuntimeEventKind as ExportedRuntimeEventKind,
    )
    from mutsukicore_runtime_python import (
        StdioJsonlBackendServer,
        run_stdio_server,
    )

    assert exported_error == "capability.exhausted"
    assert ExportedRuntimeEvent is RuntimeEvent
    assert ExportedRuntimeEventKind is RuntimeEventKind
    assert StdioJsonlBackendServer.__name__ == "StdioJsonlBackendServer"
    assert callable(run_stdio_server)
