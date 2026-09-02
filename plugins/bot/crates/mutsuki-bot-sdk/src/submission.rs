//! Flow-only submission gate for Bot business surfaces.
//!
//! Flow initiates business behavior: business plugins are invoked through graph
//! bindings or submit `mutsuki.bot.flow/ingress@1` trigger events. The gate
//! fails loud when a business `EventSource` or helper submits a platform business
//! protocol directly; the adapter and delivery service keep their own clients.

use std::sync::Arc;

use mutsuki_bot_protocol::{BOT_MESSAGE_RECALL_PROTOCOL_ID, BOT_MESSAGE_SEND_PROTOCOL_ID};
use mutsuki_runtime_contracts::{PluginManifest, RuntimeError, TaskBatch, TaskHandle, TaskOutcome};
use mutsuki_runtime_sdk::{RuntimeFailure, RuntimeResult, TaskSubmitter};

/// Protocol families that hand work to the platform or durable delivery; only
/// the adapter (via graph send nodes) and the delivery service write them.
const DENIED_PROTOCOL_PREFIXES: &[&str] = &["mutsuki.bot.delivery/", "mutsuki.bot.agent/"];

fn denied(protocol_id: &str) -> bool {
    protocol_id == BOT_MESSAGE_SEND_PROTOCOL_ID
        || protocol_id == BOT_MESSAGE_RECALL_PROTOCOL_ID
        || DENIED_PROTOCOL_PREFIXES
            .iter()
            .any(|prefix| protocol_id.starts_with(prefix))
}

fn denial(route: String) -> RuntimeFailure {
    RuntimeFailure::new(RuntimeError::new(
        mutsuki_runtime_contracts::ERR_REGISTRY_UNAUTHORIZED,
        "mutsuki.bot.sdk.submission_gate",
        route,
    ))
}

pub struct BotSubmissionGate {
    inner: Arc<dyn TaskSubmitter>,
}

impl BotSubmissionGate {
    #[must_use]
    pub fn new(inner: Arc<dyn TaskSubmitter>) -> Self {
        Self { inner }
    }

    /// Fails loud when a business manifest declares an outbound
    /// (`requires_protocol`) surface on a denied business protocol.
    pub fn ensure_manifest_business_surface(
        manifest: &PluginManifest,
    ) -> Result<(), RuntimeFailure> {
        for runner in &manifest.provides.runners {
            for surface in &runner.contract_surfaces {
                if let Some(protocol_id) = surface.as_str().strip_prefix("requires:task_protocol:")
                    && denied(protocol_id)
                {
                    return Err(denial(format!(
                        "flow_only_submission.manifest.{}.{}.requires.{protocol_id}",
                        manifest.plugin_id, runner.runner_id
                    )));
                }
            }
        }
        Ok(())
    }
}

impl TaskSubmitter for BotSubmissionGate {
    fn submit_batch(&self, batch: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
        for task in &batch.tasks {
            if denied(task.protocol_id.as_str()) {
                return Err(denial(format!(
                    "flow_only_submission.denied.{}",
                    task.protocol_id
                )));
            }
        }
        self.inner.submit_batch(batch)
    }

    fn cancel_task(&self, handle: &TaskHandle) -> RuntimeResult<()> {
        self.inner.cancel_task(handle)
    }

    fn task_outcome(&self, handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
        self.inner.task_outcome(handle)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use mutsuki_bot_protocol::BOT_FLOW_INGRESS_PROTOCOL_ID;
    use mutsuki_runtime_contracts::{CancelPolicy, Task, TaskBatch};
    use mutsuki_runtime_sdk::{PluginBuilder, RunnerDescriptorBuilder};
    use serde_json::json;

    use super::*;

    #[derive(Default)]
    struct RecordingSubmitter {
        batches: Mutex<Vec<TaskBatch>>,
    }

    impl TaskSubmitter for RecordingSubmitter {
        fn submit_batch(&self, batch: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
            let handles = batch
                .tasks
                .iter()
                .map(|task| TaskHandle {
                    task_id: task.task_id.clone(),
                    protocol_id: task.protocol_id.clone(),
                    target_binding_id: None,
                    cancel_policy: CancelPolicy::Cascade,
                    trace_id: None,
                    correlation_id: None,
                })
                .collect();
            self.batches.lock().expect("recording batches").push(batch);
            Ok(handles)
        }

        fn cancel_task(&self, _handle: &TaskHandle) -> RuntimeResult<()> {
            Ok(())
        }

        fn task_outcome(&self, _handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
            Ok(None)
        }
    }

    fn task(protocol_id: &str) -> Task {
        Task::new(format!("task.{protocol_id}"), protocol_id, json!({}))
    }

    #[test]
    fn gate_denies_direct_business_submissions_and_passes_the_rest() {
        let gate = BotSubmissionGate::new(Arc::new(RecordingSubmitter::default()));
        for protocol_id in [
            BOT_MESSAGE_SEND_PROTOCOL_ID,
            BOT_MESSAGE_RECALL_PROTOCOL_ID,
            "mutsuki.bot.delivery/reply@1",
            "mutsuki.bot.agent/submit@1",
        ] {
            let error = gate
                .submit_batch(TaskBatch::one("batch", task(protocol_id)))
                .expect_err("denied business submission");
            assert_eq!(error.error().source, "mutsuki.bot.sdk.submission_gate");
        }
        for protocol_id in [
            BOT_FLOW_INGRESS_PROTOCOL_ID,
            "mutsuki.bot.bilibili.poll/live@1",
        ] {
            let handles = gate
                .submit_batch(TaskBatch::one("batch", task(protocol_id)))
                .expect("allowed submission");
            assert_eq!(handles.len(), 1);
        }
    }

    #[test]
    fn manifest_surface_check_rejects_business_requires() {
        let violating = PluginBuilder::new("test.business.plugin")
            .runner_descriptor(
                RunnerDescriptorBuilder::new("test.runner", "test.business.plugin")
                    .accepted_protocol(BOT_FLOW_INGRESS_PROTOCOL_ID)
                    .requires_protocol(BOT_MESSAGE_SEND_PROTOCOL_ID)
                    .build(),
            )
            .build()
            .manifest;
        let error = BotSubmissionGate::ensure_manifest_business_surface(&violating)
            .expect_err("business requires denied");
        assert!(error.error().route.contains("test.business.plugin"));
        assert!(error.error().route.contains(BOT_MESSAGE_SEND_PROTOCOL_ID));

        let flow_only = PluginBuilder::new("test.business.plugin")
            .runner_descriptor(
                RunnerDescriptorBuilder::new("test.runner", "test.business.plugin")
                    .requires_protocol(BOT_FLOW_INGRESS_PROTOCOL_ID)
                    .build(),
            )
            .build()
            .manifest;
        BotSubmissionGate::ensure_manifest_business_surface(&flow_only)
            .expect("flow-only surface allowed");
    }
}
