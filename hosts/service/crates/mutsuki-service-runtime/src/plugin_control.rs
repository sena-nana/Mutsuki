//! Plugin inventory, configuration and the reload / deployment transactions.

#[allow(clippy::wildcard_imports)]
use super::*;

impl ServiceRuntimeInner {
    pub(crate) fn current_config(&self) -> ServiceConfig {
        let mut config = self.config.clone();
        config.plugins.configured = self
            .configured_selections
            .lock()
            .expect("configured selections mutex")
            .clone();
        config
    }

    pub(crate) async fn reconfigure_plugins(
        &self,
        candidates: &[ConfiguredPluginSelection],
    ) -> ServiceRuntimeResult<PluginReconfigureReport> {
        let _reload = self.reload_lock.lock().await;
        let mut affected = candidates
            .iter()
            .map(|selection| selection.id.trim().to_string())
            .collect::<BTreeSet<_>>();
        if affected.is_empty() || affected.contains("") {
            return Err(ServiceRuntimeError::EmptyConfiguredPluginId);
        }

        let mut candidate_config = self.current_config();
        candidate_config
            .plugins
            .configured
            .retain(|selection| !affected.contains(selection.id.trim()));
        candidate_config
            .plugins
            .configured
            .extend(candidates.iter().cloned());
        // Consumers of a replaced application service are rebuilt against the new provider.
        let current_services = self.application_services.snapshot();
        loop {
            let dependents = current_services.dependents_of(&affected);
            if dependents.is_subset(&affected) {
                break;
            }
            affected.extend(dependents);
        }

        let mut candidate_components = PreparedComponentRegistry::default();
        let mut created_builtin_services = BTreeMap::new();
        for plugin_id in &affected {
            let selections = candidate_config
                .plugins
                .configured
                .iter()
                .filter(|selection| selection.id.trim() == plugin_id)
                .collect::<Vec<_>>();
            let [selection] = selections.as_slice() else {
                return Err(ServiceRuntimeError::UnknownConfiguredPlugin(
                    PluginId::from(plugin_id.as_str()),
                ));
            };
            if !selection.enabled {
                continue;
            }
            if let Some(field) = raw_credential_field(&selection.config, "") {
                return Err(ServiceRuntimeError::RawConfiguredPluginSecret {
                    plugin_id: PluginId::from(plugin_id.as_str()),
                    field,
                });
            }
            let factory = self.configured_plugins.factory(plugin_id).ok_or_else(|| {
                ServiceRuntimeError::UnknownConfiguredPlugin(PluginId::from(plugin_id.as_str()))
            })?;
            let builder = ServiceRuntimeBuilder {
                config: candidate_config.clone(),
                configured_plugins: self.configured_plugins.clone(),
                components: PreparedComponentRegistry::default(),
                runtime_client: self.runtime_client.clone(),
                application_services: self.application_services.clone(),
                current_component: Some(plugin_id.clone()),
            };
            let prepared = factory
                .prepare(&selection.config, builder)
                .map_err(|detail| ServiceRuntimeError::ConfiguredPluginInstall {
                    plugin_id: PluginId::from(plugin_id.as_str()),
                    detail,
                })?;
            for (component_id, component) in prepared.components.records {
                // Application-lifetime services first created by a reconfigured consumer join
                // the Host-owned builtin component; nothing else of it may change.
                if component_id == BUILTIN_COMPONENT {
                    created_builtin_services.extend(component.application_services);
                    continue;
                }
                if !affected.contains(&component_id) {
                    return Err(ServiceRuntimeError::ConfiguredPluginInstall {
                        plugin_id: PluginId::from(plugin_id.as_str()),
                        detail: format!(
                            "configured factory emitted component owned by `{component_id}`"
                        ),
                    });
                }
                candidate_components.records.insert(component_id, component);
            }
        }

        let candidate_load_plan_hooks = candidate_components.load_plan_hooks();
        let candidate_owned_load_plan_hooks = candidate_components.owned_load_plan_hooks();
        let new_event_sources = candidate_components.take_event_sources();
        validate_event_sources(&new_event_sources, &candidate_config)?;
        let mut combined_components = self
            .components
            .lock()
            .expect("component registry mutex")
            .clone();
        let mut runtime_affected = affected.clone();
        for plugin_id in &affected {
            if let Some(component) = combined_components.records.get(plugin_id) {
                runtime_affected.extend(component.manifests.keys().map(ToString::to_string));
            }
            if let Some(component) = candidate_components.records.get(plugin_id) {
                runtime_affected.extend(component.manifests.keys().map(ToString::to_string));
            }
            combined_components.replace_component(
                plugin_id,
                candidate_components.records.get(plugin_id).cloned(),
            );
        }

        combined_components
            .record(BUILTIN_COMPONENT)
            .application_services
            .extend(created_builtin_services);
        let staged_services = StagedApplicationServices::stage(
            &self.application_services,
            ApplicationServiceTable::from_components(&combined_components)?,
        );
        let state = self
            .deployment_state
            .lock()
            .expect("deployment state mutex")
            .clone();
        let new_catalog = load_catalog_with_state(
            &candidate_config,
            &combined_components.builtin_registry(),
            &state,
        )?;
        let native_runner_factories = combined_components.native_runner_factories();
        let async_handler_factories = combined_components.async_handler_factories();
        let loaded_plugin_factories = combined_components.loaded_plugin_factories();
        let runner_limits = resolve_runner_limits(
            &combined_components.runner_limits(),
            &combined_components.runner_limit_factories(),
        );
        let load_plan_observers = combined_components.load_plan_observers();
        let previous_registry_generation = self
            .host_runtime
            .lock()
            .expect("host runtime mutex")
            .as_ref()
            .ok_or(ServiceRuntimeError::RuntimeUnavailable)?
            .host_context()
            .registry_generation();
        let registry_generation = previous_registry_generation.saturating_add(1);
        let (bootstrapper, profile) = runtime_bootstrapper(
            &candidate_config,
            &new_catalog,
            &native_runner_factories,
            &async_handler_factories,
            &loaded_plugin_factories,
            self.runtime_client.clone(),
        )
        .await?;
        let prepared = bootstrapper.prepare_targeted_reload_with_runner_limits(
            profile,
            registry_generation,
            runner_limits,
            runtime_affected.clone(),
        )?;
        let runtime_lock = prepared.load_plan().clone();
        for hook in &candidate_load_plan_hooks {
            hook.validate(&runtime_lock)
                .map_err(ServiceRuntimeError::LoadPlanHook)?;
        }
        for observer in &load_plan_observers {
            observer
                .validate(&runtime_lock)
                .map_err(ServiceRuntimeError::LoadPlanHook)?;
        }
        let runtime_lock_path = self.config.service.run_dir.join("runtime.lock.json");
        let previous_runtime_lock = fs::read(&runtime_lock_path).ok();
        write_runtime_lock(&self.config, &runtime_lock)?;
        let graceful = Duration::from_millis(self.config.runners.graceful_shutdown_ms);
        let decision = {
            let mut runtime = self.host_runtime.lock().expect("host runtime mutex");
            let runtime = runtime
                .as_mut()
                .ok_or(ServiceRuntimeError::RuntimeUnavailable)?;
            let drain_timeout = reload_drain_timeout(&candidate_config, &new_catalog);
            run_blocking_on_worker(|| runtime.reload(prepared, drain_timeout))
        };
        if decision.is_ok() {
            staged_services.commit();
        }
        if let Err(error) = decision {
            let restore_lock = restore_runtime_lock(&runtime_lock_path, previous_runtime_lock);
            return match restore_lock {
                Ok(()) => Err(ServiceRuntimeError::Core(error)),
                Err(restore) => Err(ServiceRuntimeError::PluginDeploymentState {
                    path: runtime_lock_path.display().to_string(),
                    detail: format!("{error}; restoring runtime lock failed: {restore}"),
                }),
            };
        }
        let host_lifecycle_errors = self
            .host_runtime
            .lock()
            .expect("host runtime mutex")
            .as_mut()
            .ok_or(ServiceRuntimeError::RuntimeUnavailable)?
            .take_reload_lifecycle_errors();

        let task_submitter = self
            .host_runtime
            .lock()
            .expect("host runtime mutex")
            .as_ref()
            .ok_or(ServiceRuntimeError::RuntimeUnavailable)?
            .host_context()
            .task_submitter_ref();
        let (dirty_event_source_plugins, mut lifecycle_errors) =
            self.retire_event_source_effects(runtime_affected.iter().map(String::as_str), graceful);
        lifecycle_errors.extend(host_lifecycle_errors.iter().map(runtime_error_summary));
        lifecycle_errors.extend(
            self.retire_load_plan_hook_effects(affected.iter().map(String::as_str), graceful),
        );
        for (component_id, hook) in candidate_owned_load_plan_hooks {
            match self.attach_load_plan_hook_effect(&component_id, hook.clone()) {
                Ok(()) => hook.activate(&runtime_lock),
                Err(error) => lifecycle_errors.push(format!(
                    "load-plan hook `{component_id}` could not be owned after reload: {error}"
                )),
            }
        }
        for observer in &load_plan_observers {
            observer.activate(&runtime_lock);
        }
        for source in new_event_sources {
            let descriptor = source.descriptor().clone();
            if dirty_event_source_plugins.contains(&descriptor.plugin_id) {
                lifecycle_errors.push(format!(
                    "event source `{}` was not started because plugin `{}` retains a dirty source owner",
                    descriptor.source_id, descriptor.plugin_id
                ));
                continue;
            }
            if let Err(error) = self
                .start_scoped_event_source(
                    source,
                    task_submitter.clone(),
                    &candidate_config,
                    graceful,
                )
                .await
            {
                lifecycle_errors.push(error.to_string());
            }
        }
        lifecycle_errors.extend(
            self.reconcile_scoped_sidecars(
                sidecar_specs(&candidate_config, &new_catalog),
                graceful,
                Some(&runtime_affected),
            )
            .await,
        );
        *self.catalog.lock().expect("catalog mutex") = new_catalog;
        *self.components.lock().expect("component registry mutex") = combined_components;
        *self
            .configured_selections
            .lock()
            .expect("configured selections mutex") = candidate_config.plugins.configured.clone();
        let mut generations = self
            .component_generations
            .lock()
            .expect("component generations mutex");
        let components = affected
            .into_iter()
            .map(|plugin_id| {
                let previous_generation = generations.get(&plugin_id).copied().unwrap_or(0);
                let generation = previous_generation.saturating_add(1);
                generations.insert(plugin_id.clone(), generation);
                PluginReconfigureGeneration {
                    plugin_id,
                    previous_generation,
                    generation,
                    rolled_back: false,
                }
            })
            .collect();
        let report = PluginReconfigureReport {
            components,
            registry_generation,
            runner_errors: lifecycle_errors,
        };
        self.control_changes.publish([ControlChangeDomain::Plugins]);
        Ok(report)
    }

    pub(crate) fn plugin_list(&self) -> ControlResponse {
        let catalog = self.catalog.lock().expect("catalog mutex");
        let configured = self
            .configured_selections
            .lock()
            .expect("configured selections mutex")
            .clone();
        let state = self
            .deployment_state
            .lock()
            .expect("deployment state mutex");
        let mut plugin_ids = configured
            .iter()
            .map(|selection| PluginId::from(selection.id.trim()))
            .collect::<BTreeSet<_>>();
        plugin_ids.extend(
            catalog
                .candidates
                .iter()
                .map(|record| record.manifest.plugin_id.clone()),
        );
        plugin_ids.extend(
            catalog
                .records
                .iter()
                .map(|record| record.manifest.plugin_id.clone()),
        );
        let plugins = plugin_ids
            .into_iter()
            .map(|plugin_id| {
                let active = catalog
                    .records
                    .iter()
                    .find(|record| record.manifest.plugin_id == plugin_id);
                PluginStatus {
                    configured: configured.iter().any(|selection| {
                        selection.enabled && selection.id.trim() == plugin_id.as_str()
                    }),
                    active_deployment: active
                        .map(|record| deployment_name(&deployment_for(record))),
                    preferred_deployment: state.plugins.get(&plugin_id).map(deployment_name),
                    candidates: catalog
                        .candidates
                        .iter()
                        .filter(|record| record.manifest.plugin_id == plugin_id)
                        .map(|record| PluginCandidateStatus {
                            deployment: deployment_name(&deployment_for(record)),
                            version: record.manifest.version.clone(),
                            api_version: record.manifest.api_version.clone(),
                            sha256: record.manifest.artifact.sha256.clone(),
                            path: record.manifest_path.display().to_string(),
                            available: true,
                            runner_link: record
                                .runtime
                                .as_ref()
                                .map(|runtime| runtime.runner_link.clone()),
                        })
                        .collect(),
                    plugin_id: plugin_id.to_string(),
                }
            })
            .collect();
        let diagnostics = catalog
            .diagnostics
            .iter()
            .map(|item| PluginInventoryDiagnostic {
                manifest_path: item.manifest_path.display().to_string(),
                plugin_id: item.plugin_id.as_ref().map(|id| id.to_string()),
                deployment: item.deployment.as_ref().map(deployment_name),
                detail: item.detail.clone(),
            })
            .collect();
        ControlResponse::ok(ControlResult::PluginList(PluginListResponse {
            plugins,
            diagnostics,
        }))
    }

    pub(crate) async fn plugin_reload(&self) -> ControlResponse {
        let _reload = self.reload_lock.lock().await;
        let config = self.current_config();
        let state = self
            .deployment_state
            .lock()
            .expect("deployment state mutex")
            .clone();
        let builtin_registry = self
            .components
            .lock()
            .expect("component registry mutex")
            .builtin_registry();
        let new_catalog = match load_catalog_with_state(&config, &builtin_registry, &state) {
            Ok(catalog) => catalog,
            Err(error) => return ControlResponse::err(ControlError::Failed(error.to_string())),
        };
        self.reload_catalog_unlocked(&config, new_catalog).await
    }

    pub(crate) async fn plugin_deployment_set(
        &self,
        param: PluginDeploymentParam,
    ) -> ControlResponse {
        self.change_plugin_deployment(
            param.plugin_id,
            Some(param.deployment),
            ControlResult::PluginDeploymentSet,
        )
        .await
    }

    pub(crate) async fn plugin_deployment_clear(
        &self,
        param: PluginDeploymentClearParam,
    ) -> ControlResponse {
        self.change_plugin_deployment(param.plugin_id, None, ControlResult::PluginDeploymentClear)
            .await
    }

    pub(crate) async fn change_plugin_deployment(
        &self,
        plugin_id: String,
        deployment: Option<PluginDeploymentKind>,
        result: fn(PluginReloadResponse) -> ControlResult,
    ) -> ControlResponse {
        let _reload = self.reload_lock.lock().await;
        if !self
            .configured_selections
            .lock()
            .expect("configured selections mutex")
            .iter()
            .any(|selection| selection.enabled && selection.id.trim() == plugin_id.trim())
        {
            return ControlResponse::err(ControlError::BadRequest(format!(
                "plugin {plugin_id} is not enabled by plugins.configured"
            )));
        }
        let previous = self
            .deployment_state
            .lock()
            .expect("deployment state mutex")
            .clone();
        let mut next = previous.clone();
        match deployment {
            Some(deployment) => {
                next.plugins
                    .insert(PluginId::from(plugin_id.as_str()), deployment);
            }
            None => {
                next.plugins.remove(plugin_id.as_str());
            }
        }
        let builtin_registry = self
            .components
            .lock()
            .expect("component registry mutex")
            .builtin_registry();
        let config = self.current_config();
        let new_catalog = match load_catalog_with_state(&config, &builtin_registry, &next) {
            Ok(catalog) => catalog,
            Err(error) => return ControlResponse::err(ControlError::Failed(error.to_string())),
        };
        if let Err(error) = save_deployment_state(&self.config, &next) {
            return ControlResponse::err(ControlError::Failed(error.to_string()));
        }
        let response = self.reload_catalog_unlocked(&config, new_catalog).await;
        if response.is_ok() {
            *self
                .deployment_state
                .lock()
                .expect("deployment state mutex") = next;
        } else if let Err(error) = save_deployment_state(&self.config, &previous) {
            return ControlResponse::err(ControlError::Failed(format!(
                "deployment reload failed and restoring management state failed: {error}"
            )));
        }
        match response {
            ControlResponse::Ok(ControlResult::PluginReload(response)) => {
                ControlResponse::ok(result(response))
            }
            other => other,
        }
    }

    /// Full catalog reload. `config` must be the *current* effective config
    /// (`current_config()`), never the boot-time `self.config`: configured plugin
    /// selections changed by `reconfigure_plugins` live only in the former, and
    /// re-initializing ABI plugins or sidecars from the boot snapshot would
    /// silently restore their old settings.
    pub(crate) async fn reload_catalog_unlocked(
        &self,
        config: &ServiceConfig,
        new_catalog: PluginCatalog,
    ) -> ControlResponse {
        let previous_generation = {
            let guard = self.host_runtime.lock().expect("host runtime mutex");
            let Some(runtime) = guard.as_ref() else {
                return ControlResponse::err(ControlError::CoreUnavailable);
            };
            runtime.host_context().registry_generation()
        };
        let registry_generation = previous_generation.saturating_add(1);
        let components = self
            .components
            .lock()
            .expect("component registry mutex")
            .clone();
        let native_runner_factories = components.native_runner_factories();
        let async_handler_factories = components.async_handler_factories();
        let loaded_plugin_factories = components.loaded_plugin_factories();
        let configured_runner_limits = components.runner_limits();
        let runner_limit_factories = components.runner_limit_factories();
        let load_plan_hooks = components.load_plan_hooks();
        let owned_load_plan_hooks = components.owned_load_plan_hooks();
        let load_plan_observers = components.load_plan_observers();
        let runner_limits =
            resolve_runner_limits(&configured_runner_limits, &runner_limit_factories);
        let prepared = match runtime_bootstrapper(
            config,
            &new_catalog,
            &native_runner_factories,
            &async_handler_factories,
            &loaded_plugin_factories,
            self.runtime_client.clone(),
        )
        .await
        .and_then(|(bootstrapper, profile)| {
            Ok(bootstrapper.prepare_reload_with_runner_limits(
                profile,
                registry_generation,
                runner_limits,
            )?)
        }) {
            Ok(prepared) => prepared,
            Err(error) => return ControlResponse::err(ControlError::Failed(error.to_string())),
        };
        let runtime_lock = prepared.load_plan().clone();
        for hook in &load_plan_hooks {
            if let Err(error) = hook.validate(&runtime_lock) {
                return ControlResponse::err(ControlError::Failed(error));
            }
        }
        for observer in &load_plan_observers {
            if let Err(error) = observer.validate(&runtime_lock) {
                return ControlResponse::err(ControlError::Failed(error));
            }
        }
        let drain_timeout = reload_drain_timeout(config, &new_catalog);
        let plugin_count = new_catalog.records.len();
        let sidecars = sidecar_specs(config, &new_catalog);
        let runtime_lock_path = self.config.service.run_dir.join("runtime.lock.json");
        let previous_runtime_lock = fs::read(&runtime_lock_path).ok();
        if let Err(error) = write_runtime_lock(&self.config, &runtime_lock) {
            return ControlResponse::err(ControlError::Failed(error.to_string()));
        }
        // Validate and commit under the lock, but drain without it: control calls arriving
        // during the drain reach the actor and get its structured `host.reload.busy` reply
        // instead of queueing on this mutex for the whole drain budget.
        let pending = {
            let mut guard = self.host_runtime.lock().expect("host runtime mutex");
            let Some(runtime) = guard.as_mut() else {
                return ControlResponse::err(ControlError::CoreUnavailable);
            };
            runtime.begin_reload(prepared, drain_timeout)
        };
        let reloaded = pending.and_then(|mut pending| {
            run_blocking_on_worker(|| pending.dispatch());
            let mut guard = self.host_runtime.lock().expect("host runtime mutex");
            match guard.as_mut() {
                Some(runtime) => run_blocking_on_worker(|| runtime.finish_reload(pending)),
                None => {
                    // Shutdown took the runtime while it drained; nothing is left to switch.
                    drop(guard);
                    if let Err(error) = pending.abandon() {
                        tracing::warn!(%error, "abandoned reload candidate cleanup failed");
                    }
                    return Ok(None);
                }
            }
            .map(Some)
        });
        let decision = match reloaded {
            Ok(Some(decision)) => decision,
            Ok(None) => {
                if let Err(error) = restore_runtime_lock(&runtime_lock_path, previous_runtime_lock)
                {
                    tracing::warn!(%error, "restoring runtime lock after abandoned reload failed");
                }
                return ControlResponse::err(ControlError::CoreUnavailable);
            }
            Err(error) => {
                let restore = restore_runtime_lock(&runtime_lock_path, previous_runtime_lock);
                return ControlResponse::err(ControlError::Failed(match restore {
                    Ok(()) => error.to_string(),
                    Err(restore_error) => {
                        format!("{error}; restoring runtime lock failed: {restore_error}")
                    }
                }));
            }
        };
        let host_lifecycle_errors = self
            .host_runtime
            .lock()
            .expect("host runtime mutex")
            .as_mut()
            .map(HostRuntime::take_reload_lifecycle_errors)
            .unwrap_or_default();

        let graceful = Duration::from_millis(self.config.runners.graceful_shutdown_ms);
        let old_hook_owners = self
            .load_plan_hook_effects
            .lock()
            .expect("load plan hook effect mutex")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let mut lifecycle_errors = self
            .retire_load_plan_hook_effects(old_hook_owners.iter().map(String::as_str), graceful);
        lifecycle_errors.extend(host_lifecycle_errors.iter().map(runtime_error_summary));
        for (component_id, hook) in owned_load_plan_hooks {
            match self.attach_load_plan_hook_effect(&component_id, hook.clone()) {
                Ok(()) => hook.activate(&runtime_lock),
                Err(error) => lifecycle_errors.push(format!(
                    "load-plan hook `{component_id}` could not be owned after reload: {error}"
                )),
            }
        }
        for observer in &load_plan_observers {
            observer.activate(&runtime_lock);
        }

        *self.catalog.lock().expect("catalog mutex") = new_catalog;
        lifecycle_errors.extend(
            self.reconcile_scoped_sidecars(sidecars, graceful, None)
                .await,
        );
        ControlResponse::ok(ControlResult::PluginReload(PluginReloadResponse {
            previous_generation,
            registry_generation,
            plugin_count,
            changes: decision
                .changes
                .into_iter()
                .map(|change| PluginReloadChange {
                    surface_id: change.surface_id,
                    compatibility: surface_compatibility(change.compatibility),
                })
                .collect(),
            runner_errors: lifecycle_errors,
            event_sources: "kept".into(),
        }))
    }
}

/// Candidate application services visible to factories while a reconfigure is prepared; dropping
/// without `commit` restores the previous generation.
struct StagedApplicationServices<'a> {
    directory: &'a ApplicationServiceDirectory,
    previous: Option<Arc<ApplicationServiceTable>>,
}

impl<'a> StagedApplicationServices<'a> {
    fn stage(directory: &'a ApplicationServiceDirectory, table: ApplicationServiceTable) -> Self {
        Self {
            directory,
            previous: Some(directory.replace(table)),
        }
    }

    fn commit(mut self) {
        self.previous = None;
    }
}

impl Drop for StagedApplicationServices<'_> {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            self.directory.restore(previous);
        }
    }
}
