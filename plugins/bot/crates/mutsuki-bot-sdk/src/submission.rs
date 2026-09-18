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

const DELIVERY_PREFIX: &str = "mutsuki.bot.delivery/";
const AGENT_PREFIX: &str = "mutsuki.bot.agent/";

fn denied(protocol_id: &str) -> bool {
    protocol_id == BOT_MESSAGE_SEND_PROTOCOL_ID
        || protocol_id == BOT_MESSAGE_RECALL_PROTOCOL_ID
        || DENIED_PROTOCOL_PREFIXES
            .iter()
            .any(|prefix| protocol_id.starts_with(prefix))
}

/// Which outbound surface a manifest is entitled to declare.
///
/// Rule 13 makes Flow the only initiation surface for business behavior, and
/// Rule 14 exempts the paths that drain an effect a Flow chain already started.
/// The exemption is named per call site rather than baked into a plugin-id list
/// here, so this SDK stays domain-neutral and every exemption is visible where
/// the assembly decides to grant it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BotManifestSurface {
    /// Rule 13 business plugin. Invoked only through a graph node binding, so it
    /// may not declare any outbound platform, delivery or Agent surface.
    Business,
    /// Rule 14 durable reply producer: hands an already-produced reply to the
    /// delivery service instead of sending it. May declare `mutsuki.bot.delivery/*`;
    /// still may not send or recall on the platform directly, and still may not
    /// originate an Agent turn outside a graph binding.
    DurableReplyProducer,
    /// Rule 14 effect drain: the delivery or adapter service that performs the
    /// real external send. May declare platform send/recall and delivery, and
    /// still may not originate an Agent turn.
    EffectDrain,
}

impl BotManifestSurface {
    fn forbids(self, protocol_id: &str) -> bool {
        let platform_write = protocol_id == BOT_MESSAGE_SEND_PROTOCOL_ID
            || protocol_id == BOT_MESSAGE_RECALL_PROTOCOL_ID;
        let delivery = protocol_id.starts_with(DELIVERY_PREFIX);
        let agent = protocol_id.starts_with(AGENT_PREFIX);
        match self {
            Self::Business => platform_write || delivery || agent,
            Self::DurableReplyProducer => platform_write || agent,
            Self::EffectDrain => agent,
        }
    }
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
        Self::ensure_manifest_surface(manifest, BotManifestSurface::Business)
    }

    /// Fails loud when a manifest declares an outbound (`requires_protocol`)
    /// surface wider than the one its role entitles it to.
    ///
    /// # Errors
    ///
    /// Returns `ERR_REGISTRY_UNAUTHORIZED` naming the plugin, runner and protocol.
    pub fn ensure_manifest_surface(
        manifest: &PluginManifest,
        surface: BotManifestSurface,
    ) -> Result<(), RuntimeFailure> {
        for runner in &manifest.provides.runners {
            for contract in &runner.contract_surfaces {
                if let Some(protocol_id) = contract.as_str().strip_prefix("requires:task_protocol:")
                    && surface.forbids(protocol_id)
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

    fn manifest_requiring(protocol_id: &str) -> PluginManifest {
        PluginBuilder::new("test.plugin")
            .runner_descriptor(
                RunnerDescriptorBuilder::new("test.runner", "test.plugin")
                    .requires_protocol(protocol_id)
                    .build(),
            )
            .build()
            .manifest
    }

    #[test]
    fn surface_tiers_widen_only_as_far_as_rule_14_allows() {
        use BotManifestSurface::{Business, DurableReplyProducer, EffectDrain};

        // (protocol, allowed for Business, DurableReplyProducer, EffectDrain)
        let cases = [
            (BOT_FLOW_INGRESS_PROTOCOL_ID, true, true, true),
            ("mutsuki.bot.delivery/reply@1", false, true, true),
            (BOT_MESSAGE_SEND_PROTOCOL_ID, false, false, true),
            (BOT_MESSAGE_RECALL_PROTOCOL_ID, false, false, true),
            // No tier may originate an Agent turn outside a graph binding.
            ("mutsuki.bot.agent/submit@1", false, false, false),
        ];

        for (protocol_id, business, producer, drain) in cases {
            let manifest = manifest_requiring(protocol_id);
            for (surface, allowed) in [
                (Business, business),
                (DurableReplyProducer, producer),
                (EffectDrain, drain),
            ] {
                let result = BotSubmissionGate::ensure_manifest_surface(&manifest, surface);
                assert_eq!(
                    result.is_ok(),
                    allowed,
                    "{protocol_id} under {surface:?} should be allowed={allowed}"
                );
            }
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
