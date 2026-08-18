use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mutsuki_bot_delivery::{QqDeliveryFailure, QqDeliveryGateway, QqDeliverySuccess};
use mutsuki_bot_protocol::{
    BOT_MESSAGE_SEND_PROTOCOL_ID, BotDeliveryContent, BotDeliveryPartReceipt, BotEvent, BotMessage,
    BotNodeInvocation, DeliveryPartStatus, QqConversationRef,
};
use mutsuki_bot_sandbox::{
    SandboxApi, SandboxService, is_sandbox_conversation, is_sandbox_target,
    qq_conversation_from_target,
};
use mutsuki_runtime_contracts::{
    BatchPayload, CompletionBatch, EntryCompletion, RunnerContext, RunnerDescriptor, RunnerResult,
    Task, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RuntimeResult};
use serde_json::{Value, json};

#[derive(Clone, Default)]
pub struct SandboxInterceptHandle {
    service: Arc<Mutex<Option<Arc<SandboxService>>>>,
}

impl SandboxInterceptHandle {
    pub fn bind(&self, service: Arc<SandboxService>) {
        *self.service.lock().expect("sandbox intercept mutex") = Some(service);
    }

    fn service(&self) -> Option<Arc<SandboxService>> {
        self.service
            .lock()
            .expect("sandbox intercept mutex")
            .clone()
    }

    fn record(&self, conversation: &QqConversationRef, content: &BotDeliveryContent) -> String {
        self.service()
            .and_then(|service| {
                service.observe_outbound(
                    conversation,
                    &content.segments,
                    content.reply_to.as_deref(),
                )
            })
            .map(|message| message.message_id)
            .unwrap_or_else(|| format!("sandbox-msg-{:016x}", fastrand::u64(..)))
    }

    pub fn intercept_delivery(
        &self,
        conversation: &QqConversationRef,
        content: &BotDeliveryContent,
    ) -> Option<QqDeliverySuccess> {
        if !is_sandbox_conversation(conversation) {
            return None;
        }
        let message_id = self.record(conversation, content);
        Some(QqDeliverySuccess {
            platform_message_ids: vec![message_id.clone()],
            part_receipts: vec![BotDeliveryPartReceipt {
                part_index: 0,
                status: DeliveryPartStatus::Succeeded,
                platform_message_id: Some(message_id),
                error_code: None,
            }],
        })
    }

    pub fn intercept_send(&self, account_id: &str, task: &Task) -> Option<Value> {
        if task.protocol_id != BOT_MESSAGE_SEND_PROTOCOL_ID {
            return None;
        }
        let message = send_message_from_task(task)?;
        if !is_sandbox_target(&message.target) {
            return None;
        }
        let conversation = qq_conversation_from_target(account_id, &message.target)?;
        let message_id = self.record(
            &conversation,
            &BotDeliveryContent {
                segments: message.segments,
                summary: None,
                reply_to: message.reply_to,
            },
        );
        Some(json!({ "id": message_id, "sandbox": true }))
    }
}

pub struct SandboxAwareDeliveryGateway {
    inner: Arc<dyn QqDeliveryGateway>,
    intercept: SandboxInterceptHandle,
}

impl SandboxAwareDeliveryGateway {
    pub fn new(inner: Arc<dyn QqDeliveryGateway>, intercept: SandboxInterceptHandle) -> Self {
        Self { inner, intercept }
    }
}

impl QqDeliveryGateway for SandboxAwareDeliveryGateway {
    fn send(
        &self,
        conversation: &QqConversationRef,
        content: &BotDeliveryContent,
    ) -> Result<QqDeliverySuccess, QqDeliveryFailure> {
        if let Some(success) = self.intercept.intercept_delivery(conversation, content) {
            return Ok(success);
        }
        self.inner.send(conversation, content)
    }
}

pub struct SandboxAwareOpenApiRunner {
    inner: Box<dyn Runner>,
    intercept: SandboxInterceptHandle,
    account_id: String,
}

impl SandboxAwareOpenApiRunner {
    pub fn new(
        inner: Box<dyn Runner>,
        intercept: SandboxInterceptHandle,
        account_id: impl Into<String>,
    ) -> Self {
        Self {
            inner,
            intercept,
            account_id: account_id.into(),
        }
    }
}

impl Runner for SandboxAwareOpenApiRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        self.inner.descriptor()
    }

    fn run_batch(
        &mut self,
        ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        let mut intercepted = HashMap::new();
        let mut rest_entries = Vec::new();
        let mut rest_tasks = Vec::new();
        for entry in &batch.entries {
            let task = match batch.payload_task(entry.payload_index) {
                Ok(task) => task.into_owned(),
                Err(error) => {
                    intercepted.insert(
                        entry.entry_id.clone(),
                        EntryCompletion {
                            entry_id: entry.entry_id.clone(),
                            task_id: entry.task_id.clone(),
                            result: None,
                            error: Some(error),
                        },
                    );
                    continue;
                }
            };
            if let Some(response) = self.intercept.intercept_send(&self.account_id, &task) {
                let mut result = RunnerResult::completed(task.task_id.clone());
                result.output = Some(response);
                intercepted.insert(
                    entry.entry_id.clone(),
                    EntryCompletion {
                        entry_id: entry.entry_id.clone(),
                        task_id: entry.task_id.clone(),
                        result: Some(result),
                        error: None,
                    },
                );
                continue;
            }
            let mut rest_entry = entry.clone();
            rest_entry.payload_index = rest_tasks.len();
            rest_entries.push(rest_entry);
            rest_tasks.push(task);
        }
        if !rest_tasks.is_empty() {
            let rest_batch = WorkBatch {
                batch_id: batch.batch_id.clone(),
                tick_id: batch.tick_id.clone(),
                batch_key: batch.batch_key.clone(),
                entries: rest_entries,
                payload: BatchPayload::from_tasks(&rest_tasks),
                resource_plan: batch.resource_plan.clone(),
                task_leases: batch.task_leases.clone(),
            };
            let rest = self.inner.run_batch(ctx, rest_batch)?;
            for completion in rest.results {
                intercepted.insert(completion.entry_id.clone(), completion);
            }
        }
        let results = batch
            .entries
            .iter()
            .filter_map(|entry| intercepted.remove(&entry.entry_id))
            .collect();
        Ok(CompletionBatch::from_results(&batch, results))
    }
}

fn message_from_value(value: Value) -> Option<BotMessage> {
    if let Ok(message) = serde_json::from_value::<BotMessage>(value.clone()) {
        return Some(message);
    }
    let event = serde_json::from_value::<BotEvent>(value).ok()?;
    event.message.map(|mut message| {
        message.target = event.target;
        message
    })
}

fn send_message_from_task(task: &Task) -> Option<BotMessage> {
    let value = task.payload.to_value();
    message_from_value(value.clone()).or_else(|| {
        let invocation = serde_json::from_value::<BotNodeInvocation>(value).ok()?;
        message_from_value(invocation.input.payload.value)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_bot_protocol::{
        BotConversationKind, BotTarget, MessageSegment, QQ_CONVERSATION_REF_VERSION,
    };
    use mutsuki_bot_sandbox::{SANDBOX_GROUP_ID, SandboxApi};
    use mutsuki_runtime_contracts::{
        BatchEntry, BatchKey, DispatchLane, OrderingRequirement, WorkResourcePlan,
    };
    use mutsuki_runtime_sdk::RunnerDescriptorBuilder;

    struct RecordingGateway {
        sent: Mutex<Vec<String>>,
    }

    impl QqDeliveryGateway for RecordingGateway {
        fn send(
            &self,
            conversation: &QqConversationRef,
            _content: &BotDeliveryContent,
        ) -> Result<QqDeliverySuccess, QqDeliveryFailure> {
            self.sent
                .lock()
                .expect("sent")
                .push(conversation.origin_key());
            Ok(QqDeliverySuccess {
                platform_message_ids: vec!["qq-real".into()],
                part_receipts: Vec::new(),
            })
        }
    }

    struct RecordingRunner {
        descriptor: RunnerDescriptor,
        sent: Arc<Mutex<Vec<String>>>,
    }

    impl Runner for RecordingRunner {
        fn descriptor(&self) -> &RunnerDescriptor {
            &self.descriptor
        }

        fn run_batch(
            &mut self,
            _ctx: RunnerContext,
            batch: WorkBatch,
        ) -> RuntimeResult<CompletionBatch> {
            mutsuki_runtime_sdk::map_work_batch_entries(&batch, |task| {
                self.sent
                    .lock()
                    .expect("sent")
                    .push(task.task_id.to_string());
                let mut result = RunnerResult::completed(task.task_id.clone());
                result.output = Some(json!({ "id": "qq-openapi" }));
                Ok(result)
            })
        }
    }

    fn conversation(group_id: &str) -> QqConversationRef {
        QqConversationRef {
            version: QQ_CONVERSATION_REF_VERSION,
            account_id: "qq-main".into(),
            kind: BotConversationKind::Group,
            user_id: None,
            group_id: Some(group_id.into()),
            guild_id: None,
            channel_id: None,
            thread_id: None,
        }
    }

    fn send_task(task_id: &str, group_id: &str, text: &str) -> Task {
        Task::new(
            task_id,
            BOT_MESSAGE_SEND_PROTOCOL_ID,
            serde_json::to_value(BotMessage::text(
                BotTarget::Group {
                    group_id: group_id.into(),
                },
                text,
            ))
            .unwrap(),
        )
    }

    fn work_batch(tasks: &[Task]) -> WorkBatch {
        let entries = tasks
            .iter()
            .enumerate()
            .map(|(index, task)| BatchEntry {
                entry_id: format!("entry-{index}").into(),
                task_id: task.task_id.clone(),
                trace_id: None,
                parent_id: None,
                payload_index: index,
                resource_requirement_indices: Vec::new(),
                cancel_index: None,
                deadline_tick: None,
                priority: 0,
                lane: DispatchLane::Normal,
                ordering: OrderingRequirement::PreserveSubmitOrder,
            })
            .collect();
        WorkBatch {
            batch_id: "batch".into(),
            tick_id: "tick".into(),
            batch_key: BatchKey::from("test.qq.send"),
            entries,
            payload: BatchPayload::from_tasks(tasks),
            resource_plan: WorkResourcePlan::empty(),
            task_leases: Vec::new(),
        }
    }

    fn content(text: &str) -> BotDeliveryContent {
        BotDeliveryContent {
            segments: vec![MessageSegment::text(text)],
            summary: None,
            reply_to: None,
        }
    }

    #[test]
    fn delivery_gateway_intercepts_sandbox_and_forwards_real() {
        let inner = Arc::new(RecordingGateway {
            sent: Mutex::new(Vec::new()),
        });
        let handle = SandboxInterceptHandle::default();
        handle.bind(Arc::new(SandboxService::with_account("qq-main")));
        let gateway = SandboxAwareDeliveryGateway::new(inner.clone(), handle);
        let sandbox = gateway
            .send(&conversation(SANDBOX_GROUP_ID), &content("pong"))
            .unwrap();
        assert_eq!(sandbox.platform_message_ids.len(), 1);
        assert!(inner.sent.lock().expect("sent").is_empty());
        gateway
            .send(&conversation("group-1"), &content("real"))
            .unwrap();
        assert_eq!(inner.sent.lock().expect("sent").len(), 1);
    }

    #[test]
    fn openapi_runner_intercepts_sandbox_send() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let inner = RecordingRunner {
            descriptor: RunnerDescriptorBuilder::new("test.qq.send", "test.qq")
                .accepted_protocol(BOT_MESSAGE_SEND_PROTOCOL_ID)
                .build(),
            sent: sent.clone(),
        };
        let handle = SandboxInterceptHandle::default();
        let service = Arc::new(SandboxService::with_account("qq-main"));
        handle.bind(service.clone());
        let mut runner = SandboxAwareOpenApiRunner::new(Box::new(inner), handle, "qq-main");
        let tasks = vec![
            send_task("sandbox-send", SANDBOX_GROUP_ID, "pong"),
            send_task("real-send", "group-1", "real"),
        ];
        let completion = runner
            .run_batch(
                RunnerContext::new(
                    1,
                    1,
                    "executor",
                    None::<mutsuki_runtime_contracts::TaskLeaseId>,
                    "inv",
                ),
                work_batch(&tasks),
            )
            .unwrap();
        assert_eq!(completion.results.len(), 2);
        assert_eq!(
            completion.results[0]
                .result
                .as_ref()
                .unwrap()
                .output
                .as_ref()
                .unwrap()["sandbox"],
            json!(true)
        );
        assert_eq!(
            completion.results[1]
                .result
                .as_ref()
                .unwrap()
                .output
                .as_ref()
                .unwrap()["id"],
            "qq-openapi"
        );
        assert_eq!(sent.lock().expect("sent").as_slice(), ["real-send"]);
        let messages = futures_executor::block_on(
            service.messages(&conversation(SANDBOX_GROUP_ID).origin_key()),
        )
        .unwrap();
        assert!(
            messages.iter().any(|item| item.text == "pong"
                && item.role == mutsuki_bot_sandbox::SandboxSpeakerRole::Bot)
        );
    }
}
