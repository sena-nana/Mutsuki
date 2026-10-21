//! Service, Core and Host status, metrics, health and log tail.

#[allow(clippy::wildcard_imports)]
use super::*;

impl ServiceRuntimeInner {
    pub(crate) async fn service_status(&self) -> ControlResponse {
        let runners = self.supervisor.list().await;
        let core_running = self
            .host_runtime
            .lock()
            .expect("host runtime mutex")
            .is_some();
        ControlResponse::ok(ControlResult::ServiceStatus(ServiceStatus {
            instance_id: self.config.service.instance_id.clone(),
            profile: self.config.service.profile.clone(),
            uptime_ms: self.started_at.elapsed().as_millis(),
            ipc_endpoint: self.config.ipc_endpoint(),
            core_running,
            plugin_count: self.catalog.lock().expect("catalog mutex").records.len(),
            runner_count: runners.len(),
        }))
    }

    pub(crate) fn host_metrics(&self) -> ControlResponse {
        let host_runtime = self.host_runtime.lock().expect("host runtime mutex");
        let core = host_runtime.as_ref().map(|runtime| {
            let metrics = runtime.metrics();
            CoreRuntimeMetrics {
                control_mailbox_depth: metrics.control_mailbox_depth,
                data_mailbox_depth: metrics.data_mailbox_depth,
                control_oldest_message_age_ns: duration_nanos(metrics.control_oldest_message_age),
                data_oldest_message_age_ns: duration_nanos(metrics.data_oldest_message_age),
                submit_to_dispatch_samples: metrics.submit_to_dispatch_samples,
                submit_to_dispatch_total_ns: metrics.submit_to_dispatch_total_ns,
                submit_to_dispatch_max_ns: metrics.submit_to_dispatch_max_ns,
                cancel_propagation_samples: metrics.cancel_propagation_samples,
                cancel_propagation_total_ns: metrics.cancel_propagation_total_ns,
                cancel_propagation_max_ns: metrics.cancel_propagation_max_ns,
                completion_route_samples: metrics.completion_route_samples,
                completion_route_total_ns: metrics.completion_route_total_ns,
                completion_route_max_ns: metrics.completion_route_max_ns,
                scheduler_passes: metrics.scheduler_passes,
                scheduler_total_ns: metrics.scheduler_total_ns,
                scheduler_max_ns: metrics.scheduler_max_ns,
                lane_starvation_events: metrics.lane_starvation_events,
                reserved_capacity_uses: metrics.reserved_capacity_uses,
            }
        });
        let execution_domains = host_runtime
            .as_ref()
            .and_then(|runtime| runtime.worker_pools().ok())
            .unwrap_or_default()
            .into_iter()
            .map(|domain| ExecutionDomainMetrics {
                domain_id: domain.domain_id,
                execution_classes: domain
                    .execution_classes
                    .into_iter()
                    .map(|class| format!("{class:?}").to_ascii_lowercase())
                    .collect(),
                configured_threads: domain.configured_threads,
                active_threads: domain.active_threads,
                queued_entries: domain.queued_entries,
                running_entries: domain.running_entries,
                inflight_bytes: domain.inflight_bytes,
                max_inflight_bytes: domain.max_inflight_bytes,
                degraded: domain.degraded,
                lanes: domain
                    .lanes
                    .into_iter()
                    .map(|lane| ExecutionLaneMetrics {
                        lane: format!("{:?}", lane.lane).to_ascii_lowercase(),
                        queued_entries: lane.queued_entries,
                        running_entries: lane.running_entries,
                        inflight_bytes: lane.inflight_bytes,
                        queue_entry_limit: lane.queue_entry_limit,
                        max_inflight_bytes: lane.max_inflight_bytes,
                    })
                    .collect(),
            })
            .collect();
        ControlResponse::ok(ControlResult::HostMetrics(HostMetrics {
            pid: std::process::id(),
            uptime_ms: self.started_at.elapsed().as_millis(),
            rss_bytes: current_rss_bytes(),
            cpu_time_ms: current_cpu_time_ms(),
            core,
            execution_domains,
        }))
    }

    pub(crate) fn service_shutdown(&self) -> ControlResponse {
        if let Some(tx) = self.shutdown_tx.lock().expect("shutdown mutex").take() {
            let _ = tx.send("control-api".into());
        }
        ControlResponse::ok(ControlResult::ServiceShutdown)
    }

    pub(crate) fn core_status(&self) -> ControlResponse {
        let guard = self.host_runtime.lock().expect("host runtime mutex");
        let status = guard.as_ref().map(|runtime| CoreStatus {
            running: true,
            profile_id: Some(runtime.host_context().profile_id().into()),
            registry_generation: Some(runtime.host_context().registry_generation()),
        });
        ControlResponse::ok(ControlResult::CoreStatus(status.unwrap_or(CoreStatus {
            running: false,
            profile_id: None,
            registry_generation: None,
        })))
    }

    pub(crate) fn runtime_statistics(&self) -> ControlResponse {
        let guard = self.host_runtime.lock().expect("host runtime mutex");
        let Some(runtime) = guard.as_ref() else {
            return ControlResponse::err(ControlError::CoreUnavailable);
        };
        match runtime.statistics() {
            Ok(statistics) => {
                ControlResponse::ok(ControlResult::RuntimeStatistics(RuntimeStatisticsView {
                    tasks: TaskPoolStatisticsView {
                        ready: statistics.tasks.ready,
                        running: statistics.tasks.running,
                        waiting: statistics.tasks.waiting,
                        blocked: statistics.tasks.blocked,
                        completed: statistics.tasks.completed,
                        failed: statistics.tasks.failed,
                        cancelled: statistics.tasks.cancelled,
                        expired: statistics.tasks.expired,
                        dead_letter: statistics.tasks.dead_letter,
                        submitted_total: statistics.tasks.submitted_total,
                        attempts_started: statistics.tasks.attempts_started,
                        cumulative_queue_steps: statistics.tasks.cumulative_queue_steps,
                        cumulative_execution_steps: statistics.tasks.cumulative_execution_steps,
                        stale_results_rejected: statistics.tasks.stale_results_rejected,
                        terminal_records_evicted: statistics.tasks.terminal_records_evicted,
                    },
                    retained_events: statistics.retained_events,
                    dropped_events: statistics.dropped_events,
                    retained_traces: statistics.retained_traces,
                    dropped_traces: statistics.dropped_traces,
                    scheduler_decisions: statistics.scheduler_decisions,
                }))
            }
            Err(error) => ControlResponse::err(ControlError::Failed(error.to_string())),
        }
    }

    pub(crate) async fn health_check(&self) -> ControlResponse {
        let runners = self.supervisor.list().await;
        let event_source_details = self.event_sources.list();
        let runner_health = if runners
            .iter()
            .any(|runner| matches!(runner.state, RunnerProcessState::Failed))
        {
            "degraded"
        } else {
            "ok"
        };
        let event_source_health = if event_source_details
            .iter()
            .any(|source| source.state == "failed" || source.health == "unhealthy")
        {
            "degraded"
        } else {
            "ok"
        };
        let mut recent_errors = event_source_details
            .iter()
            .filter_map(|source| {
                source
                    .last_error
                    .as_ref()
                    .map(|error| format!("event_source:{}:{error}", source.source_id))
            })
            .collect::<Vec<_>>();
        let worker_pools = self
            .host_runtime
            .lock()
            .expect("host runtime mutex")
            .as_ref()
            .and_then(|runtime| runtime.worker_pools().ok());
        let worker_degraded = worker_pools
            .as_ref()
            .is_some_and(|pools| pools.iter().any(|pool| pool.degraded));
        if worker_degraded {
            recent_errors.push("worker_pool:isolation_capacity_exhausted".into());
        }
        let mut components = self
            .components
            .lock()
            .expect("component registry mutex")
            .health_probes()
            .iter()
            .map(|(id, probe)| (id.clone(), probe()))
            .collect::<BTreeMap<_, _>>();
        if let Some(worker_pools) = worker_pools {
            components.insert(
                "worker_pools".into(),
                serde_json::to_value(worker_pools).unwrap_or(Value::Null),
            );
        }
        let report = HealthReport {
            service: "ok".into(),
            core: if self
                .host_runtime
                .lock()
                .expect("host runtime mutex")
                .is_some()
            {
                if worker_degraded {
                    "degraded".into()
                } else {
                    "ok".into()
                }
            } else {
                "stopped".into()
            },
            plugins: "ok".into(),
            runners: runner_health.into(),
            event_sources: event_source_health.into(),
            event_source_details,
            recent_errors,
            components,
        };
        ControlResponse::ok(ControlResult::HealthCheck(report))
    }

    pub(crate) fn log_tail(&self, params: LogTailParams) -> ControlResponse {
        match read_log_tail(
            self.config
                .service
                .log_dir
                .join(&self.config.observe.log_file),
            params,
        ) {
            Ok(response) => ControlResponse::ok(ControlResult::LogTail(response)),
            Err(error) => ControlResponse::err(error),
        }
    }
}
