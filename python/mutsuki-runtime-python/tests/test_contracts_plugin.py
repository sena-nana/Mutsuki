from __future__ import annotations

from mutsuki_runtime_python.contracts.plugin import (
    ArtifactType,
    HandlerBinding,
    LifecyclePolicy,
    PermissionGrant,
    PluginArtifact,
    PluginManifest,
    PluginProvides,
    ProtocolDescriptor,
    RuntimeLoadPlan,
    RuntimeProfile,
)
from mutsuki_runtime_python.contracts.runner import (
    RunnerDescriptor,
    RunnerPurity,
)
from mutsuki_runtime_python.contracts.surface import (
    ContractSurface,
    ContractSurfaceKind,
)
from mutsuki_runtime_python.testing.assertions import assert_json_roundtrip


def test_plugin_load_plan_profile_protocol_and_handler_binding_roundtrip() -> None:
    descriptor = RunnerDescriptor(
        runner_id="runner-a",
        plugin_id="plugin-a",
        plugin_generation=1,
        accepted_task_kinds=("raw.input",),
        purity=RunnerPurity.PURE,
        input_schema={"type": "object"},
        output_schema={"type": "object"},
        metadata={},
        contract_surfaces=("runner:runner-a",),
    )
    protocol = ProtocolDescriptor(
        protocol_id="im.message.received.v1",
        version="1.0.0",
        input_schema={"type": "object"},
        output_schema={"type": "object"},
        error_schema={"type": "object"},
        codec="json",
        compatibility="semver",
    )
    binding = HandlerBinding(
        binding_id="message-handler",
        plugin_id="plugin-a",
        protocol_id="im.message.received.v1",
        target_task_kind="raw.input",
        target_runner_hint="runner-a",
        pool_id="default",
        priority=5,
        policy="required",
        metadata={"rank": 1},
    )
    provides = PluginProvides(
        runners=(descriptor,),
        protocols=(protocol,),
        handler_bindings=(binding,),
        resource_schemas=("bytes.v1",),
        resource_providers=("python.resource",),
        effects=("effect.chat.send",),
        streams=("chat.events",),
        subscriptions=("chat.messages",),
        timers=("heartbeat",),
        state_schemas=("state.actor.v1",),
    )
    manifest = PluginManifest(
        plugin_id="plugin-a",
        version="0.1.0",
        api_version="mutsuki-plugin-v1",
        artifact=PluginArtifact(
            artifact_type=ArtifactType.PYTHON,
            path="plugin.py",
            sha256="sha256:plugin",
        ),
        provides=provides,
        requires=(),
        permissions=PermissionGrant(effects=("effect.chat.send",), resources=("read",)),
        lifecycle=LifecyclePolicy(
            reload_policy="drain_and_swap",
            unload_timeout_ms=5000,
            supports_cancel=True,
            supports_dispose=True,
            supports_snapshot=False,
        ),
        metadata={"rank": 1},
    )
    plan = RuntimeLoadPlan(
        lock_version=1,
        core_api_version="mutsuki-core-v1",
        profile_id="default",
        profile_hash="sha256:profile",
        registry_generation=1,
        plugins=(manifest,),
        load_order=("plugin-a",),
        runner_bindings={"raw.input": "runner-a"},
        contract_surfaces=(
            ContractSurface(
                surface_id="runner:runner-a",
                kind=ContractSurfaceKind.RUNNER,
                owner_plugin_id="plugin-a",
                fingerprint="sha256:runner",
                deprecated=False,
            ),
            ContractSurface(
                surface_id="protocol:im.message.received.v1",
                kind=ContractSurfaceKind.PROTOCOL,
                owner_plugin_id="plugin-a",
                fingerprint="protocol:im.message.received.v1:1.0.0",
                deprecated=False,
            ),
            ContractSurface(
                surface_id="handler_binding:message-handler",
                kind=ContractSurfaceKind.HANDLER_BINDING,
                owner_plugin_id="plugin-a",
                fingerprint="handler_binding:message-handler",
                deprecated=False,
            ),
        ),
    )

    assert_json_roundtrip(ProtocolDescriptor, protocol)
    assert_json_roundtrip(HandlerBinding, binding)
    assert_json_roundtrip(PluginProvides, provides)
    assert_json_roundtrip(PluginManifest, manifest)
    assert_json_roundtrip(RuntimeLoadPlan, plan)
    assert_json_roundtrip(
        RuntimeProfile,
        RuntimeProfile(
            profile_id="default",
            enabled_plugins=("plugin-a",),
            bindings={"raw.input": "plugin-a"},
            allow_dynamic_registration=False,
            allow_hot_reload=True,
        ),
    )

