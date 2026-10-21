//! Host effects whose lifetime follows a plugin scope: load-plan hooks, sidecars and event sources.

#[allow(clippy::wildcard_imports)]
use super::*;

impl ServiceRuntimeInner {
    pub(crate) fn attach_load_plan_hook_effect(
        &self,
        component_id: &str,
        hook: Arc<dyn LoadPlanLifecycleHook>,
    ) -> ServiceRuntimeResult<()> {
        let handle = self
            .host_runtime
            .lock()
            .expect("host runtime mutex")
            .as_ref()
            .ok_or(ServiceRuntimeError::RuntimeUnavailable)?
            .attach_application_effect(
                component_id,
                HostEffectKind::HostLocal,
                Box::new(LoadPlanHookScopeEffect(hook)),
            )?;
        self.load_plan_hook_effects
            .lock()
            .expect("load plan hook effect mutex")
            .entry(component_id.to_string())
            .or_default()
            .push(handle);
        Ok(())
    }

    pub(crate) fn retire_load_plan_hook_effects<'a>(
        &self,
        component_ids: impl IntoIterator<Item = &'a str>,
        timeout: Duration,
    ) -> Vec<String> {
        let mut effects = self
            .load_plan_hook_effects
            .lock()
            .expect("load plan hook effect mutex");
        let handles = component_ids
            .into_iter()
            .flat_map(|component_id| {
                effects
                    .remove(component_id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(move |handle| (component_id.to_string(), handle))
            })
            .collect::<Vec<_>>();
        drop(effects);
        let runtime = self.host_runtime.lock().expect("host runtime mutex");
        let Some(runtime) = runtime.as_ref() else {
            return handles
                .into_iter()
                .map(|(component_id, _)| {
                    format!(
                        "load-plan hook `{component_id}` could not be retired: runtime unavailable"
                    )
                })
                .collect();
        };
        handles
            .into_iter()
            .filter_map(|(component_id, handle)| {
                runtime
                    .dispose_attached_effect(&handle, timeout)
                    .err()
                    .map(|error| format!("load-plan hook `{component_id}`: {error}"))
            })
            .collect()
    }

    pub(crate) fn attach_sidecar_effect(
        &self,
        spec: &ManagedRunnerSpec,
        graceful: Duration,
    ) -> ServiceRuntimeResult<()> {
        let handle = self
            .host_runtime
            .lock()
            .expect("host runtime mutex")
            .as_ref()
            .ok_or(ServiceRuntimeError::RuntimeUnavailable)?
            .attach_plugin_effect(
                &spec.plugin_id,
                HostEffectKind::BackendInstance,
                Box::new(RunnerSupervisorScopeEffect {
                    supervisor: self.supervisor.clone(),
                    runner_id: spec.runner_id.clone(),
                    graceful,
                }),
            )?;
        self.sidecar_effects
            .lock()
            .expect("sidecar effect mutex")
            .insert(spec.runner_id.clone(), (spec.plugin_id.clone(), handle));
        Ok(())
    }

    pub(crate) fn prune_disposed_sidecar_effects(&self) -> Vec<String> {
        let runtime = self.host_runtime.lock().expect("host runtime mutex");
        let Some(runtime) = runtime.as_ref() else {
            return Vec::new();
        };
        let mut effects = self.sidecar_effects.lock().expect("sidecar effect mutex");
        effects.retain(|_, (_, handle)| runtime.attached_effect_state(handle).is_some());
        effects.keys().cloned().collect()
    }

    pub(crate) async fn reconcile_scoped_sidecars(
        &self,
        desired: Vec<ManagedRunnerSpec>,
        graceful: Duration,
        reloaded_plugins: Option<&BTreeSet<String>>,
    ) -> Vec<String> {
        let live = self.prune_disposed_sidecar_effects();
        let protected = {
            let effects = self.sidecar_effects.lock().expect("sidecar effect mutex");
            live.into_iter()
                .filter(|runner_id| {
                    reloaded_plugins.is_none_or(|plugins| {
                        effects
                            .get(runner_id)
                            .is_some_and(|(plugin_id, _)| plugins.contains(plugin_id))
                    })
                })
                .collect::<Vec<_>>()
        };
        if !protected.is_empty() {
            return protected
                .into_iter()
                .map(|runner_id| format!("runner {runner_id} remains owned by a dirty old scope"))
                .collect();
        }

        let mut errors =
            reconcile_supervised_sidecars(desired.clone(), &self.supervisor, graceful).await;
        let running = self
            .supervisor
            .list()
            .await
            .into_iter()
            .filter(|snapshot| matches!(snapshot.state, RunnerProcessState::Running))
            .map(|snapshot| snapshot.runner_id)
            .collect::<BTreeSet<_>>();
        let owned = self
            .sidecar_effects
            .lock()
            .expect("sidecar effect mutex")
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        for spec in desired {
            if !running.contains(&spec.runner_id) || owned.contains(&spec.runner_id) {
                continue;
            }
            if let Err(error) = self.attach_sidecar_effect(&spec, graceful) {
                let _ = self.supervisor.remove(&spec.runner_id, graceful).await;
                errors.push(error.to_string());
            }
        }
        errors
    }

    pub(crate) fn attach_event_source_effect(
        &self,
        descriptor: &HostEventSourceDescriptor,
        graceful: Duration,
    ) -> ServiceRuntimeResult<()> {
        let handle = self
            .host_runtime
            .lock()
            .expect("host runtime mutex")
            .as_ref()
            .ok_or(ServiceRuntimeError::RuntimeUnavailable)?
            .attach_application_effect(
                &descriptor.plugin_id,
                HostEffectKind::HostLocal,
                Box::new(EventSourceScopeEffect {
                    supervisor: self.event_sources.clone(),
                    source_id: descriptor.source_id.clone(),
                    graceful,
                }),
            )?;
        self.event_source_effects
            .lock()
            .expect("event source effect mutex")
            .insert(
                descriptor.source_id.clone(),
                (descriptor.plugin_id.clone(), handle),
            );
        Ok(())
    }

    pub(crate) async fn start_scoped_event_source(
        &self,
        source: Box<dyn HostEventSource>,
        task_submitter: Arc<dyn TaskSubmitter>,
        config: &ServiceConfig,
        graceful: Duration,
    ) -> ServiceRuntimeResult<()> {
        let descriptor = source.descriptor().clone();
        self.event_sources
            .start(source, task_submitter, config, graceful);
        if let Err(error) = self.attach_event_source_effect(&descriptor, graceful) {
            return match self
                .event_sources
                .shutdown_source(&descriptor.source_id, graceful)
                .await
            {
                Ok(()) => Err(error),
                Err(cleanup) => Err(ServiceRuntimeError::EventSource(format!(
                    "{error}; event source `{}` cleanup failed: {cleanup}",
                    descriptor.source_id
                ))),
            };
        }
        Ok(())
    }

    pub(crate) fn retire_event_source_effects<'a>(
        &self,
        plugin_ids: impl IntoIterator<Item = &'a str>,
        timeout: Duration,
    ) -> (BTreeSet<String>, Vec<String>) {
        let plugin_ids = plugin_ids
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        let effects = self
            .event_source_effects
            .lock()
            .expect("event source effect mutex");
        let handles = effects
            .iter()
            .filter(|(_, (plugin_id, _))| plugin_ids.contains(plugin_id))
            .map(|(source_id, (plugin_id, handle))| {
                (source_id.clone(), plugin_id.clone(), handle.clone())
            })
            .collect::<Vec<_>>();
        drop(effects);
        let runtime = self.host_runtime.lock().expect("host runtime mutex");
        let Some(runtime) = runtime.as_ref() else {
            return (
                plugin_ids,
                vec!["event sources could not be retired: runtime unavailable".into()],
            );
        };
        let mut failed_plugins = BTreeSet::new();
        let mut errors = Vec::new();
        for (source_id, plugin_id, handle) in handles {
            match runtime.dispose_attached_effect(&handle, timeout) {
                Ok(()) => {
                    self.event_source_effects
                        .lock()
                        .expect("event source effect mutex")
                        .remove(&source_id);
                }
                Err(error) => {
                    failed_plugins.insert(plugin_id);
                    errors.push(format!("event source `{source_id}`: {error}"));
                }
            }
        }
        (failed_plugins, errors)
    }
}
