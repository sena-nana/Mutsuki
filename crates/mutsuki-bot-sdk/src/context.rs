use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use mutsuki_bot_protocol::{
    BOT_MEDIA_UPLOAD_PROTOCOL_ID, BOT_MESSAGE_RECALL_PROTOCOL_ID, BOT_MESSAGE_SEND_PROTOCOL_ID,
    BotMediaUploadRequest, BotMessage, BotMessageRecallRequest, BotTarget,
};
use mutsuki_runtime_contracts::{
    CancelPolicy, DispatchLane, OrderingRequirement, Task, TaskBatch, TaskHandle, TaskOutcome,
};
use mutsuki_runtime_sdk::{
    RuntimeClientRef, RuntimeFailure, TaskSubmitter, TaskSubmitterRuntimeClient,
};
use serde::Serialize;
use thiserror::Error;

use crate::MessageBuilder;

#[derive(Debug, Error)]
pub enum BotSdkError {
    #[error("payload serialization failed: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error(transparent)]
    Runtime(#[from] RuntimeFailure),
    #[error("runtime returned an invalid task handle set for {0}")]
    InvalidHandles(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BotTaskOptions {
    trace_id: Option<String>,
    correlation_id: Option<String>,
    target_binding_id: Option<String>,
    runner_hint: Option<String>,
    cancel_policy: Option<CancelPolicy>,
    priority: Option<i64>,
    dispatch_lane: Option<DispatchLane>,
    ordering: Option<OrderingRequirement>,
}

impl BotTaskOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn trace_id(mut self, trace_id: impl Into<String>) -> Self {
        self.trace_id = Some(trace_id.into());
        self
    }

    pub fn correlation_id(mut self, correlation_id: impl Into<String>) -> Self {
        self.correlation_id = Some(correlation_id.into());
        self
    }

    pub fn target_binding_id(mut self, target_binding_id: impl Into<String>) -> Self {
        self.target_binding_id = Some(target_binding_id.into());
        self
    }

    pub fn runner_hint(mut self, runner_hint: impl Into<String>) -> Self {
        self.runner_hint = Some(runner_hint.into());
        self
    }

    pub fn cancel_policy(mut self, cancel_policy: CancelPolicy) -> Self {
        self.cancel_policy = Some(cancel_policy);
        self
    }

    pub fn priority(mut self, priority: i64) -> Self {
        self.priority = Some(priority);
        self
    }

    pub fn dispatch_lane(mut self, dispatch_lane: DispatchLane) -> Self {
        self.dispatch_lane = Some(dispatch_lane);
        self
    }

    pub fn ordering(mut self, ordering: OrderingRequirement) -> Self {
        self.ordering = Some(ordering);
        self
    }

    fn resolve(&self, defaults: &Self, generated_task_id: String) -> ResolvedBotTaskOptions {
        ResolvedBotTaskOptions {
            task_id: generated_task_id,
            trace_id: self.trace_id.clone().or_else(|| defaults.trace_id.clone()),
            correlation_id: self
                .correlation_id
                .clone()
                .or_else(|| defaults.correlation_id.clone()),
            target_binding_id: self
                .target_binding_id
                .clone()
                .or_else(|| defaults.target_binding_id.clone()),
            runner_hint: self
                .runner_hint
                .clone()
                .or_else(|| defaults.runner_hint.clone()),
            cancel_policy: self
                .cancel_policy
                .clone()
                .or_else(|| defaults.cancel_policy.clone())
                .unwrap_or(CancelPolicy::Cascade),
            priority: self.priority.or(defaults.priority).unwrap_or(0),
            dispatch_lane: self
                .dispatch_lane
                .clone()
                .or_else(|| defaults.dispatch_lane.clone())
                .unwrap_or(DispatchLane::Normal),
            ordering: self
                .ordering
                .clone()
                .or_else(|| defaults.ordering.clone())
                .unwrap_or(OrderingRequirement::None),
        }
    }
}

struct ResolvedBotTaskOptions {
    task_id: String,
    trace_id: Option<String>,
    correlation_id: Option<String>,
    target_binding_id: Option<String>,
    runner_hint: Option<String>,
    cancel_policy: CancelPolicy,
    priority: i64,
    dispatch_lane: DispatchLane,
    ordering: OrderingRequirement,
}

impl ResolvedBotTaskOptions {
    fn task(&self, protocol_id: &str, payload: serde_json::Value) -> Task {
        let mut task = Task::new(&self.task_id, protocol_id, payload);
        task.trace_id = self.trace_id.clone();
        task.correlation_id = self.correlation_id.clone();
        task.target_binding_id = self.target_binding_id.clone();
        task.runner_hint = self.runner_hint.clone();
        task.priority = self.priority;
        task.dispatch_lane = self.dispatch_lane.clone();
        task.ordering = self.ordering.clone();
        task
    }
}

pub struct BotTask {
    task: Task,
    cancel_policy: CancelPolicy,
}

impl BotTask {
    pub fn task(&self) -> &Task {
        &self.task
    }
}

pub struct BotContext {
    client: RuntimeClientRef,
    task_id_prefix: String,
    next_task_id: AtomicU64,
    defaults: BotTaskOptions,
}

impl BotContext {
    pub fn new(client: RuntimeClientRef, task_id_prefix: impl Into<String>) -> Self {
        Self {
            client,
            task_id_prefix: task_id_prefix.into(),
            next_task_id: AtomicU64::new(0),
            defaults: BotTaskOptions::default(),
        }
    }

    pub fn from_submitter(
        submitter: Arc<dyn TaskSubmitter>,
        task_id_prefix: impl Into<String>,
    ) -> Self {
        let client = TaskSubmitterRuntimeClient::new(submitter).into_runtime_client();
        Self::new(client, task_id_prefix)
    }

    pub fn with_default_options(mut self, options: BotTaskOptions) -> Self {
        self.defaults = options;
        self
    }

    pub fn send_message(&self, message: BotMessage) -> Result<TaskHandle, BotSdkError> {
        self.send_message_with_options(message, BotTaskOptions::default())
    }

    pub fn send_message_with_options(
        &self,
        message: BotMessage,
        options: BotTaskOptions,
    ) -> Result<TaskHandle, BotSdkError> {
        self.submit_operation(self.prepare_message(message, options)?)
    }

    pub fn send_text(
        &self,
        target: BotTarget,
        text: impl Into<String>,
    ) -> Result<TaskHandle, BotSdkError> {
        self.send_message(MessageBuilder::new(target).text(text).build())
    }

    pub fn send_text_with_options(
        &self,
        target: BotTarget,
        text: impl Into<String>,
        options: BotTaskOptions,
    ) -> Result<TaskHandle, BotSdkError> {
        self.submit_operation(self.prepare_text(target, text, options)?)
    }

    pub fn upload_media(&self, payload: BotMediaUploadRequest) -> Result<TaskHandle, BotSdkError> {
        self.upload_media_with_options(payload, BotTaskOptions::default())
    }

    pub fn upload_media_with_options(
        &self,
        payload: BotMediaUploadRequest,
        options: BotTaskOptions,
    ) -> Result<TaskHandle, BotSdkError> {
        self.submit_operation(self.prepare_media_upload(payload, options)?)
    }

    pub fn recall_message(
        &self,
        payload: BotMessageRecallRequest,
    ) -> Result<TaskHandle, BotSdkError> {
        self.recall_message_with_options(payload, BotTaskOptions::default())
    }

    pub fn recall_message_with_options(
        &self,
        payload: BotMessageRecallRequest,
        options: BotTaskOptions,
    ) -> Result<TaskHandle, BotSdkError> {
        self.submit_operation(self.prepare_recall(payload, options)?)
    }

    pub fn prepare_message(
        &self,
        message: BotMessage,
        options: BotTaskOptions,
    ) -> Result<BotTask, BotSdkError> {
        self.prepare(BOT_MESSAGE_SEND_PROTOCOL_ID, message, options)
    }

    pub fn prepare_text(
        &self,
        target: BotTarget,
        text: impl Into<String>,
        options: BotTaskOptions,
    ) -> Result<BotTask, BotSdkError> {
        self.prepare_message(MessageBuilder::new(target).text(text).build(), options)
    }

    pub fn prepare_media_upload(
        &self,
        payload: BotMediaUploadRequest,
        options: BotTaskOptions,
    ) -> Result<BotTask, BotSdkError> {
        self.prepare(BOT_MEDIA_UPLOAD_PROTOCOL_ID, payload, options)
    }

    pub fn prepare_recall(
        &self,
        payload: BotMessageRecallRequest,
        options: BotTaskOptions,
    ) -> Result<BotTask, BotSdkError> {
        self.prepare(BOT_MESSAGE_RECALL_PROTOCOL_ID, payload, options)
    }

    pub fn submit_task(&self, task: Task) -> Result<TaskHandle, BotSdkError> {
        Ok(self.client.submit_task(task)?)
    }

    pub fn submit_batch(&self, batch: TaskBatch) -> Result<Vec<TaskHandle>, BotSdkError> {
        let handles = self.client.submit_batch(batch)?;
        let policy = self
            .defaults
            .cancel_policy
            .clone()
            .unwrap_or(CancelPolicy::Cascade);
        Ok(handles
            .into_iter()
            .map(|mut handle| {
                handle.cancel_policy = policy.clone();
                handle
            })
            .collect())
    }

    pub fn submit_operation(&self, operation: BotTask) -> Result<TaskHandle, BotSdkError> {
        let mut handle = self.client.submit_task(operation.task)?;
        handle.cancel_policy = operation.cancel_policy;
        Ok(handle)
    }

    pub fn submit_operations(
        &self,
        batch_id: impl Into<String>,
        operations: impl IntoIterator<Item = BotTask>,
    ) -> Result<Vec<TaskHandle>, BotSdkError> {
        let mut tasks = Vec::new();
        let mut cancel_policies = BTreeMap::new();
        for operation in operations {
            cancel_policies.insert(operation.task.task_id.clone(), operation.cancel_policy);
            tasks.push(operation.task);
        }
        let batch = TaskBatch {
            batch_id: batch_id.into(),
            tick_id: None,
            tasks,
            resource_plan: None,
        };
        let mut handles = self.client.submit_batch(batch)?;
        validate_handle_set(&handles, &cancel_policies)?;
        for handle in &mut handles {
            handle.cancel_policy = cancel_policies[&handle.task_id].clone();
        }
        Ok(handles)
    }

    pub fn task_outcome(&self, handle: &TaskHandle) -> Result<Option<TaskOutcome>, BotSdkError> {
        Ok(self.client.task_outcome(handle)?)
    }

    fn prepare<T>(
        &self,
        protocol_id: &str,
        payload: T,
        options: BotTaskOptions,
    ) -> Result<BotTask, BotSdkError>
    where
        T: Serialize,
    {
        let sequence = self.next_task_id.fetch_add(1, Ordering::Relaxed) + 1;
        let generated_task_id = format!("{}:{sequence}", self.task_id_prefix);
        let options = options.resolve(&self.defaults, generated_task_id);
        let task = options.task(protocol_id, serde_json::to_value(payload)?);
        Ok(BotTask {
            task,
            cancel_policy: options.cancel_policy,
        })
    }
}

fn validate_handle_set(
    handles: &[TaskHandle],
    policies: &BTreeMap<String, CancelPolicy>,
) -> Result<(), BotSdkError> {
    if handles.len() == policies.len()
        && handles
            .iter()
            .all(|handle| policies.contains_key(&handle.task_id))
    {
        return Ok(());
    }
    Err(BotSdkError::InvalidHandles(
        policies.keys().cloned().collect::<Vec<_>>().join(","),
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use mutsuki_bot_protocol::BotMediaKind;
    use mutsuki_runtime_contracts::{
        ResourceAccess, ResourceId, ResourceLifetime, ResourceRef, ResourceSealState,
        ResourceSemantic,
    };
    use mutsuki_runtime_sdk::{RuntimeClient, RuntimeResult};
    use serde_json::json;

    use super::*;

    #[derive(Default)]
    struct RecordingRuntimeClient {
        batches: Mutex<Vec<TaskBatch>>,
        outcomes: Mutex<BTreeMap<String, TaskOutcome>>,
    }

    impl RuntimeClient for RecordingRuntimeClient {
        fn submit_batch(&self, batch: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
            let handles = batch
                .tasks
                .iter()
                .map(|task| TaskHandle {
                    task_id: task.task_id.clone(),
                    protocol_id: task.protocol_id.clone(),
                    target_binding_id: task.target_binding_id.clone(),
                    cancel_policy: CancelPolicy::Cascade,
                    trace_id: task.trace_id.clone(),
                    correlation_id: task.correlation_id.clone(),
                })
                .collect();
            self.batches.lock().unwrap().push(batch);
            Ok(handles)
        }

        fn task_outcome(&self, handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
            Ok(self.outcomes.lock().unwrap().get(&handle.task_id).cloned())
        }
    }

    #[test]
    fn single_operation_returns_runtime_handle_and_propagates_metadata() {
        let client = Arc::new(RecordingRuntimeClient::default());
        let context = BotContext::new(client.clone(), "bot-test").with_default_options(
            BotTaskOptions::new()
                .trace_id("trace-1")
                .correlation_id("correlation-1"),
        );
        let handle = context
            .send_text_with_options(
                user_target(),
                "hello",
                BotTaskOptions::new()
                    .target_binding_id("binding.qqbot")
                    .runner_hint("runner.qqbot")
                    .cancel_policy(CancelPolicy::Shield)
                    .priority(42)
                    .dispatch_lane(DispatchLane::Interactive)
                    .ordering(OrderingRequirement::StrictSequence {
                        sequence_id: "conversation-1".into(),
                    }),
            )
            .unwrap();

        assert_eq!(handle.task_id, "bot-test:1");
        assert_eq!(handle.protocol_id, BOT_MESSAGE_SEND_PROTOCOL_ID);
        assert_eq!(handle.target_binding_id.as_deref(), Some("binding.qqbot"));
        assert_eq!(handle.cancel_policy, CancelPolicy::Shield);
        assert_eq!(handle.trace_id.as_deref(), Some("trace-1"));
        assert_eq!(handle.correlation_id.as_deref(), Some("correlation-1"));
        let batches = client.batches.lock().unwrap();
        let task = &batches[0].tasks[0];
        assert_eq!(task.runner_hint.as_deref(), Some("runner.qqbot"));
        assert_eq!(task.priority, 42);
        assert_eq!(task.dispatch_lane, DispatchLane::Interactive);
        assert_eq!(
            task.ordering,
            OrderingRequirement::StrictSequence {
                sequence_id: "conversation-1".into()
            }
        );
    }

    #[test]
    fn bot_batch_submits_message_media_and_recall_in_one_runtime_batch() {
        let client = Arc::new(RecordingRuntimeClient::default());
        let context = BotContext::new(client.clone(), "bot-batch").with_default_options(
            BotTaskOptions::new()
                .trace_id("trace-batch")
                .cancel_policy(CancelPolicy::Detach),
        );
        let operations = vec![
            context
                .prepare_text(user_target(), "hello", BotTaskOptions::new())
                .unwrap(),
            context
                .prepare_media_upload(media_request(), BotTaskOptions::new())
                .unwrap(),
            context
                .prepare_recall(recall_request(), BotTaskOptions::new())
                .unwrap(),
        ];

        let handles = context
            .submit_operations("operations-1", operations)
            .unwrap();

        assert_eq!(handles.len(), 3);
        assert!(
            handles
                .iter()
                .all(|handle| handle.cancel_policy == CancelPolicy::Detach)
        );
        let batches = client.batches.lock().unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].batch_id, "operations-1");
        assert_eq!(
            batches[0]
                .tasks
                .iter()
                .map(|task| task.protocol_id.as_str())
                .collect::<Vec<_>>(),
            [
                BOT_MESSAGE_SEND_PROTOCOL_ID,
                BOT_MEDIA_UPLOAD_PROTOCOL_ID,
                BOT_MESSAGE_RECALL_PROTOCOL_ID,
            ]
        );
        assert!(
            batches[0]
                .tasks
                .iter()
                .all(|task| task.trace_id.as_deref() == Some("trace-batch"))
        );
    }

    #[test]
    fn task_outcome_is_queried_through_runtime_client() {
        let client = Arc::new(RecordingRuntimeClient::default());
        let context = BotContext::new(client.clone(), "bot-outcome");
        let handle = context.send_text(user_target(), "hello").unwrap();
        client.outcomes.lock().unwrap().insert(
            handle.task_id.clone(),
            TaskOutcome::Completed {
                task_id: handle.task_id.clone(),
                output: None,
                output_ref: Some("message-result".into()),
            },
        );

        assert_eq!(
            context.task_outcome(&handle).unwrap(),
            Some(TaskOutcome::Completed {
                task_id: handle.task_id,
                output: None,
                output_ref: Some("message-result".into()),
            })
        );
    }

    fn user_target() -> BotTarget {
        BotTarget::User {
            user_id: "user-1".into(),
        }
    }

    fn media_request() -> BotMediaUploadRequest {
        BotMediaUploadRequest {
            target: user_target(),
            kind: BotMediaKind::File,
            resource: ResourceRef {
                ref_id: "media-1".into(),
                resource_id: ResourceId {
                    kind_id: "bytes".into(),
                    slot_id: "media-1".into(),
                    generation: 1,
                    version: 1,
                },
                semantic: ResourceSemantic::FrozenValue,
                provider_id: "memory".into(),
                resource_kind: "bytes".into(),
                schema: "bytes.v1".into(),
                generation: 1,
                version: 1,
                access: ResourceAccess::Blob {
                    store_id: "memory".into(),
                    key: "media-1".into(),
                },
                size_hint: None,
                content_hash: None,
                lifetime: ResourceLifetime::BorrowedUntilTaskEnd,
                lease: None,
                seal_state: ResourceSealState::Sealed,
            },
            file_name: Some("file.bin".into()),
        }
    }

    fn recall_request() -> BotMessageRecallRequest {
        BotMessageRecallRequest {
            target: user_target(),
            message_id: "message-1".into(),
        }
    }

    #[test]
    fn task_batch_facade_accepts_runtime_task_batch() {
        let client = Arc::new(RecordingRuntimeClient::default());
        let context = BotContext::new(client.clone(), "raw-batch")
            .with_default_options(BotTaskOptions::new().cancel_policy(CancelPolicy::Shield));
        let task = Task::new("raw-1", BOT_MESSAGE_SEND_PROTOCOL_ID, json!({}));

        let handles = context
            .submit_batch(TaskBatch::one("raw-batch-1", task))
            .unwrap();

        assert_eq!(handles.len(), 1);
        assert_eq!(handles[0].task_id, "raw-1");
        assert_eq!(handles[0].cancel_policy, CancelPolicy::Shield);
    }
}
