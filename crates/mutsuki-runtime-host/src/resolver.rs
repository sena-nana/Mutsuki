mod capabilities;
mod plugins;
mod surfaces;

use mutsuki_runtime_contracts::{PluginManifest, RuntimeLoadPlan, RuntimeProfile};
use mutsuki_runtime_core::RuntimeResult;

use self::capabilities::capability_graph_for;
pub(crate) use self::plugins::core_manifest;
use self::plugins::{profile_hash, resolve_enabled_plugins, runner_bindings};
use self::surfaces::surfaces_for;

pub fn resolve_load_plan(
    manifests: &[PluginManifest],
    profile: &RuntimeProfile,
) -> RuntimeResult<RuntimeLoadPlan> {
    let resolved = resolve_enabled_plugins(manifests, profile)?;
    let runner_bindings = runner_bindings(profile, &resolved.manifests)?;
    let capability_graph =
        capability_graph_for(profile, &resolved.manifests, &resolved.deployments)?;
    let contract_surfaces = surfaces_for(&resolved.manifests, &capability_graph);
    Ok(RuntimeLoadPlan {
        lock_version: 1,
        core_api_version: "mutsuki-core-v1".into(),
        profile_id: profile.profile_id.clone(),
        profile_hash: profile_hash(profile, resolved.deployments.len(), &capability_graph),
        registry_generation: 1,
        plugins: resolved.manifests,
        load_order: profile.enabled_plugins.clone(),
        runner_bindings,
        plugin_deployments: resolved.deployments,
        observability: profile.observability.clone(),
        capability_graph,
        contract_surfaces,
    })
}
