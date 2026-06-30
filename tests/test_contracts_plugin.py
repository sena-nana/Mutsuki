from __future__ import annotations

from mutsuki_runner_kit.contracts.extension import (
    BridgeDescriptor,
    CodecDescriptor,
    HostExtensionDescriptor,
    HostExtensionKind,
    PluginBackendDescriptor,
    PluginDeploymentKind,
    SchedulerPolicyDescriptor,
    WorkflowDescriptor,
)
from mutsuki_runner_kit.contracts.plugin import (
    ArtifactType,
    CapabilityProviderSelection,
    HandlerBinding,
    LifecyclePolicy,
    PermissionAuditEntry,
    PermissionGrant,
    PluginArtifact,
    PluginManifest,
    PluginProvides,
    ProtocolDescriptor,
    RuntimeCapabilityGraph,
    RuntimeLoadPlan,
    RuntimeProfile,
    RuntimeProfileMode,
)
from mutsuki_runner_kit.contracts.resource import (
    ResourceProviderCompatibility,
    ResourceProviderReloadPolicy,
    ResourceSemantic,
    ResourceTypeDescriptor,
)
from mutsuki_runner_kit.contracts.runner import (
    ExecutionClass,
    RunnerDescriptor,
    RunnerPurity,
)
from mutsuki_runner_kit.contracts.surface import (
    ContractSurface,
    ContractSurfaceKind,
)
from mutsuki_runner_kit.testing.assertions import assert_json_roundtrip


def test_plugin_load_plan_profile_protocol_and_handler_binding_roundtrip() -> None:
    descriptor = RunnerDescriptor(
        runner_id="runner-a",
        plugin_id="plugin-a",
        plugin_generation=1,
        accepted_protocol_ids=("raw.input",),
        purity=RunnerPurity.PURE,
        execution_class=ExecutionClass.CPU,
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
        target_protocol_id="raw.input",
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
        resource_types=(
            ResourceTypeDescriptor(
                kind_id="bytes",
                semantic=ResourceSemantic.FROZEN_VALUE,
                schema="bytes.v1",
                provider_id="python.resource",
                operations=("read", "export"),
                reload_policy=ResourceProviderReloadPolicy.COMPATIBLE_WITHOUT_LEASES,
                compatibility=ResourceProviderCompatibility(
                    schema_version="1.0.0",
                    required_operations=("read", "export"),
                    preserves_resource_type_id=True,
                    accepts_older_generations=True,
                    lease_drain_required=True,
                ),
            ),
        ),
        effects=("effect.chat.send",),
        streams=("chat.events",),
        subscriptions=("chat.messages",),
        timers=("heartbeat",),
        state_schemas=("state.actor.v1",),
        host_extensions=(
            HostExtensionDescriptor(
                extension_id="host.extension.python",
                kind=HostExtensionKind.PLUGIN_BACKEND,
                supported_deployments=(PluginDeploymentKind.PYTHON,),
                reload_policy="drain_and_swap",
                drain_required=True,
            ),
        ),
        plugin_backends=(
            PluginBackendDescriptor(
                backend_id="plugin.backend.python",
                deployment_kind=PluginDeploymentKind.PYTHON,
                task_client_protocol="mutsuki.task.v1",
                resource_client_protocol="mutsuki.resource-plan.v1",
                codec_id="codec.json",
                bridge_id="bridge.python.jsonl",
            ),
        ),
        codecs=(
            CodecDescriptor(
                codec_id="codec.json",
                media_type="application/json",
                version="1.0.0",
                connection_scoped=True,
            ),
        ),
        bridges=(
            BridgeDescriptor(
                bridge_id="bridge.python.jsonl",
                deployment_kind=PluginDeploymentKind.PYTHON,
                codec_ids=("codec.json",),
                drain_policy="connection_drain",
            ),
        ),
        scheduler_policies=(
            SchedulerPolicyDescriptor(
                policy_id="scheduler.fair",
                version="1.0.0",
                decision_scope="dispatch_budget",
            ),
        ),
        workflows=(
            WorkflowDescriptor(
                workflow_id="workflow.linear",
                state_resource_kind="workflow.instance",
                runner_protocol_id="workflow.linear.run",
                reload_policy="state_resource_handoff",
            ),
        ),
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
        plugin_deployments={"plugin-a": PluginDeploymentKind.PYTHON},
        capability_graph=RuntimeCapabilityGraph(
            profile_mode=RuntimeProfileMode.FULL_DEV,
            provided_capabilities=(
                "runner:runner-a",
                "task_protocol:raw.input",
                "plugin_backend:plugin.backend.python",
            ),
            required_capabilities=(),
            active_capabilities=(
                "plugin:plugin-a",
                "runner:runner-a",
                "task_protocol:raw.input",
                "resource_provider:mutsuki.std.resource.memory",
                "plugin_backend:plugin.backend.python",
            ),
            active_capability_providers=(
                CapabilityProviderSelection(
                    capability="plugin:plugin-a",
                    provider_plugin_id="plugin-a",
                    provider_version="0.1.0",
                    surface_id="plugin:plugin-a",
                    reason="active_plan",
                ),
                CapabilityProviderSelection(
                    capability="plugin_backend:plugin.backend.python",
                    provider_plugin_id="plugin-a",
                    provider_version=None,
                    surface_id="plugin_backend:plugin.backend.python",
                    reason="active_plan",
                ),
            ),
            active_resource_providers=("mutsuki.std.resource.memory",),
            active_host_extensions=("host.extension.python",),
            active_plugin_backends=("plugin.backend.python",),
            active_codecs=("codec.json",),
            active_bridges=("bridge.python.jsonl",),
            active_scheduler_policies=("scheduler.fair",),
            active_workflows=("workflow.linear",),
            permission_audit=(
                PermissionAuditEntry(
                    plugin_id="plugin-a",
                    permission_kind="effect",
                    permission="effect.chat.send",
                    granted=True,
                    provider_capability="effect:effect.chat.send",
                    reason="active_effect",
                ),
                PermissionAuditEntry(
                    plugin_id="plugin-a",
                    permission_kind="resource",
                    permission="read",
                    granted=True,
                    provider_capability="resource_type:bytes",
                    reason="active_resource",
                ),
            ),
        ),
        contract_surfaces=(
            ContractSurface(
                surface_id="runner:runner-a",
                kind=ContractSurfaceKind.RUNNER,
                owner_plugin_id="plugin-a",
                fingerprint="sha256:runner",
                deprecated=False,
            ),
            ContractSurface(
                surface_id="task_protocol:raw.input",
                kind=ContractSurfaceKind.TASK_PROTOCOL,
                owner_plugin_id="plugin-a",
                fingerprint="task_protocol:raw.input",
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
            ContractSurface(
                surface_id="plugin_backend:plugin.backend.python",
                kind=ContractSurfaceKind.PLUGIN_BACKEND,
                owner_plugin_id="plugin-a",
                fingerprint="plugin_backend:plugin.backend.python",
                deprecated=False,
            ),
            ContractSurface(
                surface_id="host_extension:host.extension.python",
                kind=ContractSurfaceKind.HOST_EXTENSION,
                owner_plugin_id="plugin-a",
                fingerprint="host_extension:host.extension.python",
                deprecated=False,
            ),
            ContractSurface(
                surface_id="codec:codec.json",
                kind=ContractSurfaceKind.CODEC,
                owner_plugin_id="plugin-a",
                fingerprint="codec:codec.json",
                deprecated=False,
            ),
            ContractSurface(
                surface_id="bridge:bridge.python.jsonl",
                kind=ContractSurfaceKind.BRIDGE,
                owner_plugin_id="plugin-a",
                fingerprint="bridge:bridge.python.jsonl",
                deprecated=False,
            ),
            ContractSurface(
                surface_id="scheduler_policy:scheduler.fair",
                kind=ContractSurfaceKind.SCHEDULER_POLICY,
                owner_plugin_id="plugin-a",
                fingerprint="scheduler_policy:scheduler.fair",
                deprecated=False,
            ),
            ContractSurface(
                surface_id="workflow:workflow.linear",
                kind=ContractSurfaceKind.WORKFLOW,
                owner_plugin_id="plugin-a",
                fingerprint="workflow:workflow.linear",
                deprecated=False,
            ),
        ),
    )

    assert_json_roundtrip(ProtocolDescriptor, protocol)
    assert_json_roundtrip(HandlerBinding, binding)
    assert_json_roundtrip(HostExtensionDescriptor, provides.host_extensions[0])
    assert_json_roundtrip(PluginBackendDescriptor, provides.plugin_backends[0])
    assert_json_roundtrip(
        PluginBackendDescriptor,
        PluginBackendDescriptor(
            backend_id="plugin.backend.builtin",
            deployment_kind=PluginDeploymentKind.BUILTIN,
            task_client_protocol="mutsuki.task.v1",
            resource_client_protocol="mutsuki.resource-plan.v1",
            codec_id=None,
            bridge_id=None,
        ),
    )
    assert_json_roundtrip(CodecDescriptor, provides.codecs[0])
    assert_json_roundtrip(BridgeDescriptor, provides.bridges[0])
    assert_json_roundtrip(SchedulerPolicyDescriptor, provides.scheduler_policies[0])
    assert_json_roundtrip(WorkflowDescriptor, provides.workflows[0])
    assert_json_roundtrip(
        CapabilityProviderSelection,
        plan.capability_graph.active_capability_providers[0],
    )
    assert_json_roundtrip(PermissionAuditEntry, plan.capability_graph.permission_audit[0])
    assert_json_roundtrip(PluginProvides, provides)
    assert_json_roundtrip(PluginManifest, manifest)
    assert_json_roundtrip(RuntimeCapabilityGraph, plan.capability_graph)
    assert_json_roundtrip(RuntimeLoadPlan, plan)
    assert_json_roundtrip(
        RuntimeProfile,
        RuntimeProfile(
            profile_id="default",
            mode=RuntimeProfileMode.FULL_DEV,
            enabled_plugins=("plugin-a",),
            bindings={"raw.input": "plugin-a"},
            plugin_deployments={"plugin-a": PluginDeploymentKind.PYTHON},
            allow_dynamic_registration=False,
            allow_hot_reload=True,
        ),
    )


def test_plugin_load_plan_rejects_missing_required_fields() -> None:
    graph = {
        "profile_mode": "full_dev",
        "provided_capabilities": [],
        "required_capabilities": [],
        "active_capabilities": [],
        "active_capability_providers": [],
        "active_resource_providers": [],
        "active_host_extensions": [],
        "active_plugin_backends": [],
        "active_codecs": [],
        "active_bridges": [],
        "active_scheduler_policies": [],
        "active_workflows": [],
        "permission_audit": [],
    }
    plan = {
        "lock_version": 1,
        "core_api_version": "mutsuki-core-v1",
        "profile_id": "default",
        "profile_hash": "sha256:profile",
        "registry_generation": 1,
        "plugins": [],
        "load_order": [],
        "runner_bindings": {},
        "plugin_deployments": {},
        "capability_graph": graph,
        "contract_surfaces": [],
    }

    missing_deployments = dict(plan)
    del missing_deployments["plugin_deployments"]
    try:
        RuntimeLoadPlan.from_json_dict(missing_deployments)
    except TypeError:
        pass
    else:
        raise AssertionError("missing plugin_deployments should fail")

    missing_provider_audit = dict(graph)
    del missing_provider_audit["active_capability_providers"]
    missing_graph_plan = dict(plan)
    missing_graph_plan["capability_graph"] = missing_provider_audit
    try:
        RuntimeLoadPlan.from_json_dict(missing_graph_plan)
    except TypeError:
        pass
    else:
        raise AssertionError("missing active_capability_providers should fail")
