use crate::approval::ApprovalBridge;
use crate::config::MutsukiTauriConfig;
use crate::error::{HostError, HostResult};
use mutsuki_runtime_contracts::{
    RunnerDescriptor, RuntimeEvent, RuntimeEventKind, ScalarValue, TaskOutcome, TaskStatus,
};
use mutsuki_runtime_host::{HostRuntime, HostRuntimeCommand, HostRuntimeReply};
use mutsuki_tauri_bridge::{
    ApprovalRequest, ApprovalResponse, FrontendContext, FrontendTaskRequest, FrontendTaskResult,
    HostStatus, MutsukiFrontendEvent, PluginSummary, PreviewHandle, ResourceBytes, ResourceText,
    TaskCancelRequest,
};
use mutsuki_tauri_resource::TauriResourceStore;
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

pub struct MutsukiTauriHost {
    config: MutsukiTauriConfig,
    runtime: Mutex<HostRuntime>,
    resources: Arc<TauriResourceStore>,
    events: Arc<mutsuki_tauri_bridge::EventHub>,
    approvals: ApprovalBridge,
    runners: Vec<RunnerDescriptor>,
}

impl MutsukiTauriHost {
    pub(crate) fn new(
        config: MutsukiTauriConfig,
        runtime: HostRuntime,
        resources: Arc<TauriResourceStore>,
        events: Arc<mutsuki_tauri_bridge::EventHub>,
        runners: Vec<RunnerDescriptor>,
    ) -> Self {
        Self {
            config,
            runtime: Mutex::new(runtime),
            resources,
            events,
            approvals: ApprovalBridge::default(),
            runners,
        }
    }

    pub fn builder() -> crate::MutsukiTauriHostBuilder {
        crate::MutsukiTauriHostBuilder::new()
    }

    pub fn config(&self) -> &MutsukiTauriConfig {
        &self.config
    }

    pub fn event_hub(&self) -> Arc<mutsuki_tauri_bridge::EventHub> {
        self.events.clone()
    }

    pub fn resource_store(&self) -> Arc<TauriResourceStore> {
        self.resources.clone()
    }

    pub fn status(&self) -> HostStatus {
        HostStatus {
            app_name: self.config.app_name.clone(),
            profile_id: self.config.profile_id.clone(),
            mode: format!("{:?}", self.config.mode).to_lowercase(),
            healthy: true,
            plugins: self.plugins(),
        }
    }

    pub fn plugins(&self) -> Vec<PluginSummary> {
        self.runners
            .iter()
            .map(|runner| PluginSummary {
                plugin_id: runner.plugin_id.clone(),
                version: format!("generation-{}", runner.plugin_generation),
                enabled: true,
                deployment: "builtin".into(),
            })
            .collect()
    }

    pub fn call(&self, request: FrontendTaskRequest) -> HostResult<FrontendTaskResult> {
        let task = request.into_task();
        let task_id = task.task_id.clone();
        let submit_event = runtime_event("task.submit", Some(task_id.clone()), BTreeMap::new());
        let _ = self.events.emit(MutsukiFrontendEvent::Task {
            task_id: task_id.clone(),
            event: submit_event.clone(),
        });
        let mut runtime = self.runtime.lock();
        let submitted = runtime.dispatch(HostRuntimeCommand::SubmitTask(Box::new(task)))?;
        match submitted {
            HostRuntimeReply::TaskSubmitted(_) => {}
            _ => return Err(HostError::Runtime("unexpected submit reply".into())),
        }
        runtime.dispatch(HostRuntimeCommand::RunUntilIdle {
            max_ticks: self.config.max_ticks_per_call,
        })?;
        let status = runtime.task_status(&task_id);
        let outcome = match runtime.dispatch(HostRuntimeCommand::TaskOutcome(task_id.clone()))? {
            HostRuntimeReply::TaskOutcome(outcome) => outcome,
            _ => return Err(HostError::Runtime("unexpected outcome reply".into())),
        };
        let terminal_name = match &outcome {
            Some(TaskOutcome::Completed { .. }) => "task.completed",
            Some(TaskOutcome::Failed { .. }) => "task.failed",
            Some(TaskOutcome::Cancelled { .. }) => "task.cancelled",
            Some(TaskOutcome::Expired { .. }) => "task.expired",
            Some(TaskOutcome::DeadLetter { .. }) => "task.dead_lettered",
            None => "task.pending",
        };
        let terminal_event = runtime_event(terminal_name, Some(task_id.clone()), BTreeMap::new());
        let _ = self.events.emit(MutsukiFrontendEvent::Task {
            task_id: task_id.clone(),
            event: terminal_event.clone(),
        });
        Ok(FrontendTaskResult {
            task_id,
            status,
            outcome,
            events: vec![submit_event, terminal_event],
        })
    }

    pub fn cancel_task(&self, request: TaskCancelRequest) -> HostResult<String> {
        let mut runtime = self.runtime.lock();
        let reply = runtime.dispatch(HostRuntimeCommand::CancelTask(request.task_id.clone()))?;
        match reply {
            HostRuntimeReply::TaskCancelled(task_id) => {
                let mut attrs = BTreeMap::new();
                if let Some(reason) = request.reason {
                    attrs.insert("reason".into(), ScalarValue::String(reason));
                }
                let event = runtime_event("task.cancelled", Some(task_id.clone()), attrs);
                let _ = self.events.emit(MutsukiFrontendEvent::Task {
                    task_id: task_id.clone(),
                    event,
                });
                Ok(task_id)
            }
            _ => Err(HostError::Runtime("unexpected cancel reply".into())),
        }
    }

    pub fn task_status(&self, task_id: &str) -> Option<TaskStatus> {
        self.runtime.lock().task_status(task_id)
    }

    pub async fn import_file(
        &self,
        path: impl AsRef<Path>,
    ) -> HostResult<mutsuki_runtime_contracts::ResourceRef> {
        let resource = self.resources.import_file(path).await?;
        let _ = self.events.emit(MutsukiFrontendEvent::Resource {
            ref_id: resource.ref_id.clone(),
            operation: "import_file".into(),
        });
        Ok(resource)
    }

    pub async fn read_resource_bytes(&self, ref_id: &str) -> HostResult<ResourceBytes> {
        let resource = self.resources.descriptor(ref_id)?;
        let bytes = self.resources.read_bytes(ref_id).await?;
        Ok(ResourceBytes {
            media_type: None,
            resource,
            bytes,
        })
    }

    pub async fn read_resource_text(&self, ref_id: &str) -> HostResult<ResourceText> {
        Ok(ResourceText {
            ref_id: ref_id.into(),
            text: self.resources.read_text(ref_id).await?,
        })
    }

    pub async fn write_resource_bytes(
        &self,
        ref_id: &str,
        bytes: Vec<u8>,
    ) -> HostResult<mutsuki_runtime_contracts::ResourceRef> {
        let resource = self.resources.write_bytes(ref_id, bytes).await?;
        let _ = self.events.emit(MutsukiFrontendEvent::Resource {
            ref_id: ref_id.into(),
            operation: "write".into(),
        });
        Ok(resource)
    }

    pub async fn export_resource_to_file(
        &self,
        ref_id: &str,
        target: impl AsRef<Path>,
    ) -> HostResult<()> {
        self.resources.export_to_file(ref_id, target).await?;
        let _ = self.events.emit(MutsukiFrontendEvent::Resource {
            ref_id: ref_id.into(),
            operation: "export_file".into(),
        });
        Ok(())
    }

    pub fn create_preview_handle(&self, ref_id: &str) -> HostResult<PreviewHandle> {
        Ok(self
            .resources
            .create_preview_handle(ref_id, Duration::from_secs(self.config.preview_ttl_secs))?)
    }

    pub fn request_approval(
        &self,
        requester: impl Into<String>,
        operation: impl Into<String>,
        risk: impl Into<String>,
        payload: Value,
        context: FrontendContext,
    ) -> ApprovalRequest {
        let request = self
            .approvals
            .request(requester, operation, risk, payload, context);
        let _ = self.events.emit(MutsukiFrontendEvent::Approval {
            request: request.clone(),
        });
        request
    }

    pub fn resolve_approval(
        &self,
        response: ApprovalResponse,
    ) -> HostResult<mutsuki_tauri_bridge::ApprovalDecision> {
        self.approvals.resolve(response)
    }

    pub fn pending_approvals(&self) -> Vec<ApprovalRequest> {
        self.approvals.pending()
    }
}

fn runtime_event(
    name: impl Into<String>,
    subject_id: Option<String>,
    attributes: BTreeMap<String, ScalarValue>,
) -> RuntimeEvent {
    RuntimeEvent {
        sequence: 0,
        kind: RuntimeEventKind::Task,
        name: name.into(),
        subject_id,
        attributes,
        error: None,
    }
}
