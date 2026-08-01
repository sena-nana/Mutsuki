use mutsuki_agent_contracts::{ScheduleExecutionStatus, ScheduledRunResult};
use mutsuki_bot_protocol::{
    BOT_ACTIVE_DELIVERY_PROTOCOL_ID, BotActiveDeliveryCommand, BotActiveDeliveryRequest,
    BotDeliveryAttempt, BotDeliveryContent, BotDeliveryPartReceipt, BotDeliveryReceipt,
    DeliveryPolicy, DeliveryStatus, MessageSegment, QqConversationRef,
};
use mutsuki_runtime_contracts::{
    CompletionBatch, ExecutionClass, PluginManifest, RunnerResult, Task, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeResult};
use mutsuki_runtime_sdk::{
    PluginBuilder, ProtocolDescriptorBuilder, RunnerDescriptorBuilder, map_work_batch_entries,
};
use std::sync::Arc;
use thiserror::Error;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ScheduledDeliveryRequest {
    pub result: ScheduledRunResult,
    pub now_unix_ms: u64,
}

pub trait DeliveryRepository: Send + Sync {
    fn reserve(
        &self,
        request: &BotActiveDeliveryRequest,
    ) -> Result<Option<BotDeliveryReceipt>, DeliveryError>;
    fn request(&self, delivery_id: &str) -> Result<BotActiveDeliveryRequest, DeliveryError>;
    fn receipt(&self, delivery_id: &str) -> Result<BotDeliveryReceipt, DeliveryError>;
    fn attempts(&self, delivery_id: &str) -> Result<Vec<BotDeliveryAttempt>, DeliveryError>;
    fn save_attempt(&self, attempt: BotDeliveryAttempt) -> Result<(), DeliveryError>;
    fn save_receipt(&self, receipt: BotDeliveryReceipt) -> Result<(), DeliveryError>;
    fn due_delivery_ids(&self, now_unix_ms: u64) -> Result<Vec<String>, DeliveryError>;
}

pub trait QqDeliveryGateway: Send + Sync {
    fn send(
        &self,
        conversation: &QqConversationRef,
        content: &BotDeliveryContent,
    ) -> Result<QqDeliverySuccess, QqDeliveryFailure>;
}

pub trait DeliveryPolicyResolver: Send + Sync {
    fn active_delivery_allowed(
        &self,
        conversation: &QqConversationRef,
    ) -> Result<bool, DeliveryError>;
}

#[derive(Clone)]
pub struct ActiveDeliveryService {
    repository: Arc<dyn DeliveryRepository>,
    gateway: Arc<dyn QqDeliveryGateway>,
    policy: Arc<dyn DeliveryPolicyResolver>,
}

impl ActiveDeliveryService {
    pub fn new(
        repository: Arc<dyn DeliveryRepository>,
        gateway: Arc<dyn QqDeliveryGateway>,
        policy: Arc<dyn DeliveryPolicyResolver>,
    ) -> Self {
        Self {
            repository,
            gateway,
            policy,
        }
    }

    pub fn submit(
        &self,
        request: &BotActiveDeliveryRequest,
        now_unix_ms: u64,
    ) -> Result<BotDeliveryReceipt, DeliveryError> {
        validate_request(request)?;
        if !self.policy.active_delivery_allowed(&request.conversation)? {
            return Err(DeliveryError::PolicyDenied);
        }
        if request.dry_run {
            return Ok(receipt(
                request,
                DeliveryStatus::Previewed,
                0,
                Vec::new(),
                Vec::new(),
                None,
                None,
            ));
        }
        if let Some(existing) = self.repository.reserve(request)? {
            return Ok(existing);
        }
        self.attempt(request, now_unix_ms, 1)
    }

    pub fn resume_due(&self, now_unix_ms: u64) -> Result<Vec<BotDeliveryReceipt>, DeliveryError> {
        self.repository
            .due_delivery_ids(now_unix_ms)?
            .into_iter()
            .filter_map(|delivery_id| match self.repository.receipt(&delivery_id) {
                Ok(receipt) if receipt.status == DeliveryStatus::RetryScheduled => {
                    Some(Ok(delivery_id))
                }
                Ok(_) | Err(DeliveryError::NotFound) => None,
                Err(error) => Some(Err(error)),
            })
            .map(|delivery_id| {
                let delivery_id = delivery_id?;
                let request = self.repository.request(&delivery_id)?;
                let attempt = self.next_attempt(&delivery_id)?;
                self.attempt(&request, now_unix_ms, attempt)
            })
            .collect()
    }

    pub fn inspect(
        &self,
        delivery_id: &str,
    ) -> Result<(BotDeliveryReceipt, Vec<BotDeliveryAttempt>), DeliveryError> {
        Ok((
            self.repository.receipt(delivery_id)?,
            self.repository.attempts(delivery_id)?,
        ))
    }

    pub fn preview(&self, delivery_id: &str) -> Result<BotDeliveryReceipt, DeliveryError> {
        let mut request = self.repository.request(delivery_id)?;
        request.dry_run = true;
        self.submit(&request, 0)
    }

    pub fn retry(
        &self,
        delivery_id: &str,
        now_unix_ms: u64,
    ) -> Result<BotDeliveryReceipt, DeliveryError> {
        let current = self.repository.receipt(delivery_id)?;
        if !matches!(
            current.status,
            DeliveryStatus::RetryScheduled | DeliveryStatus::PermanentlyFailed
        ) {
            return Err(DeliveryError::InvalidState);
        }
        let request = self.repository.request(delivery_id)?;
        if !self.policy.active_delivery_allowed(&request.conversation)? {
            return Err(DeliveryError::PolicyDenied);
        }
        self.attempt(
            &request,
            now_unix_ms,
            current.attempt_count.saturating_add(1),
        )
    }

    pub fn cancel(&self, delivery_id: &str) -> Result<BotDeliveryReceipt, DeliveryError> {
        let mut current = self.repository.receipt(delivery_id)?;
        if current.status != DeliveryStatus::RetryScheduled {
            return Err(DeliveryError::InvalidState);
        }
        current.status = DeliveryStatus::Cancelled;
        current.error_code = None;
        self.repository.save_receipt(current.clone())?;
        Ok(current)
    }

    fn next_attempt(&self, delivery_id: &str) -> Result<u32, DeliveryError> {
        Ok(self
            .repository
            .receipt(delivery_id)?
            .attempt_count
            .saturating_add(1))
    }

    fn attempt(
        &self,
        request: &BotActiveDeliveryRequest,
        now_unix_ms: u64,
        attempt_number: u32,
    ) -> Result<BotDeliveryReceipt, DeliveryError> {
        if request
            .policy
            .not_before_unix_ms
            .is_some_and(|not_before| now_unix_ms < not_before)
        {
            let receipt = receipt(
                request,
                DeliveryStatus::RetryScheduled,
                attempt_number.saturating_sub(1),
                Vec::new(),
                Vec::new(),
                None,
                None,
            );
            self.repository.save_attempt(BotDeliveryAttempt {
                delivery_id: request.delivery_id.clone(),
                attempt: attempt_number.saturating_sub(1),
                status: DeliveryStatus::RetryScheduled,
                started_at_unix_ms: now_unix_ms,
                retry_at_unix_ms: request.policy.not_before_unix_ms,
                error_code: None,
            })?;
            self.repository.save_receipt(receipt.clone())?;
            return Ok(receipt);
        }
        if request
            .policy
            .expires_at_unix_ms
            .is_some_and(|expires| now_unix_ms >= expires)
        {
            return self.finish_failure(
                request,
                attempt_number,
                "delivery.expired",
                Vec::new(),
                Vec::new(),
            );
        }
        self.repository.save_attempt(BotDeliveryAttempt {
            delivery_id: request.delivery_id.clone(),
            attempt: attempt_number,
            status: DeliveryStatus::Sending,
            started_at_unix_ms: now_unix_ms,
            retry_at_unix_ms: None,
            error_code: None,
        })?;
        match self.gateway.send(&request.conversation, &request.content) {
            Ok(success) => {
                let receipt = receipt(
                    request,
                    DeliveryStatus::Succeeded,
                    attempt_number,
                    success.platform_message_ids,
                    success.part_receipts,
                    Some(now_unix_ms),
                    None,
                );
                self.repository.save_receipt(receipt.clone())?;
                Ok(receipt)
            }
            Err(failure)
                if failure.transient && attempt_number < request.policy.max_attempts.max(1) =>
            {
                let exponential = request
                    .policy
                    .initial_backoff_ms
                    .saturating_mul(1_u64 << attempt_number.saturating_sub(1).min(20));
                let delay = failure
                    .retry_after_ms
                    .unwrap_or(exponential)
                    .min(request.policy.max_backoff_ms);
                let retry_at = now_unix_ms.saturating_add(delay);
                self.repository.save_attempt(BotDeliveryAttempt {
                    delivery_id: request.delivery_id.clone(),
                    attempt: attempt_number,
                    status: DeliveryStatus::RetryScheduled,
                    started_at_unix_ms: now_unix_ms,
                    retry_at_unix_ms: Some(retry_at),
                    error_code: Some(failure.code.clone()),
                })?;
                let receipt = receipt(
                    request,
                    DeliveryStatus::RetryScheduled,
                    attempt_number,
                    failure.sent_message_ids,
                    failure.part_receipts,
                    None,
                    Some(failure.code),
                );
                self.repository.save_receipt(receipt.clone())?;
                Ok(receipt)
            }
            Err(failure) => self.finish_failure(
                request,
                attempt_number,
                &failure.code,
                failure.sent_message_ids,
                failure.part_receipts,
            ),
        }
    }

    fn finish_failure(
        &self,
        request: &BotActiveDeliveryRequest,
        attempts: u32,
        code: &str,
        sent_message_ids: Vec<String>,
        part_receipts: Vec<BotDeliveryPartReceipt>,
    ) -> Result<BotDeliveryReceipt, DeliveryError> {
        let receipt = receipt(
            request,
            DeliveryStatus::PermanentlyFailed,
            attempts,
            sent_message_ids,
            part_receipts,
            None,
            Some(code.into()),
        );
        self.repository.save_receipt(receipt.clone())?;
        Ok(receipt)
    }
}

pub const SCHEDULE_TARGET_KIND_BOT_CONVERSATION_BINDING: &str = "mutsuki.bot.conversation_binding";
pub const BOT_SCHEDULED_DELIVERY_PROTOCOL_ID: &str = "mutsuki.bot.delivery/scheduled-result@1";
pub const BOT_SCHEDULED_DELIVERY_PLUGIN_ID: &str = "mutsuki.plugin.bot.delivery.scheduled";
pub const BOT_SCHEDULED_DELIVERY_RUNNER_ID: &str = "mutsuki.bot.delivery.scheduled.runner";

pub trait ScheduledDeliveryTargetResolver: Send + Sync {
    fn resolve_binding(&self, binding_id: &str) -> Result<QqConversationRef, DeliveryError>;
}

pub trait ScheduledDeliveryPolicyProvider: Send + Sync {
    fn delivery_policy(
        &self,
        conversation: &QqConversationRef,
    ) -> Result<DeliveryPolicy, DeliveryError>;
}

#[derive(Clone)]
pub struct ScheduledAgentDeliveryBridge {
    delivery: ActiveDeliveryService,
    targets: Arc<dyn ScheduledDeliveryTargetResolver>,
    policies: Arc<dyn ScheduledDeliveryPolicyProvider>,
}

impl ScheduledAgentDeliveryBridge {
    pub fn new(
        delivery: ActiveDeliveryService,
        targets: Arc<dyn ScheduledDeliveryTargetResolver>,
        policies: Arc<dyn ScheduledDeliveryPolicyProvider>,
    ) -> Self {
        Self {
            delivery,
            targets,
            policies,
        }
    }

    pub fn deliver(
        &self,
        result: ScheduledRunResult,
        now_unix_ms: u64,
    ) -> Result<BotDeliveryReceipt, DeliveryError> {
        if result.status != ScheduleExecutionStatus::Succeeded {
            return Err(DeliveryError::ScheduledRunNotSucceeded);
        }
        let target = result.target.ok_or(DeliveryError::InvalidScheduleTarget)?;
        if target.kind != SCHEDULE_TARGET_KIND_BOT_CONVERSATION_BINDING
            || target.target_id.trim().is_empty()
        {
            return Err(DeliveryError::InvalidScheduleTarget);
        }
        // `metadata` is deliberately ignored: only an owner-persisted opaque binding may resolve
        // to a QQ ConversationRef.
        let conversation = self.targets.resolve_binding(&target.target_id)?;
        let policy = self.policies.delivery_policy(&conversation)?;
        let mut segments = Vec::new();
        if !result.summary.trim().is_empty() {
            segments.push(MessageSegment::text(result.summary.clone()));
        }
        if let Some(resource) = result.output_ref {
            let segment = if resource.schema.starts_with("image/") {
                MessageSegment::Image { resource }
            } else if resource.schema.starts_with("audio/") {
                MessageSegment::Audio { resource }
            } else if resource.schema.starts_with("video/") {
                MessageSegment::Video { resource }
            } else {
                MessageSegment::File {
                    resource,
                    name: Some(format!("scheduled-output-{}", result.execution_id)),
                }
            };
            segments.push(segment);
        }
        let stable_id = format!("agent-scheduled:{}", result.execution_id);
        let request = BotActiveDeliveryRequest {
            delivery_id: stable_id.clone(),
            idempotency_key: stable_id,
            conversation,
            content: BotDeliveryContent {
                segments,
                summary: (!result.summary.trim().is_empty()).then_some(result.summary),
            },
            policy,
            dry_run: false,
            source_execution_id: Some(result.execution_id),
        };
        self.delivery.submit(&request, now_unix_ms)
    }
}

#[must_use]
pub fn bot_scheduled_delivery_manifest() -> PluginManifest {
    PluginBuilder::new(BOT_SCHEDULED_DELIVERY_PLUGIN_ID)
        .runner_descriptor(scheduled_delivery_descriptor())
        .protocol_handler(
            ProtocolDescriptorBuilder::new(BOT_SCHEDULED_DELIVERY_PROTOCOL_ID).build(),
            BOT_SCHEDULED_DELIVERY_RUNNER_ID,
            "bot-scheduled-delivery",
        )
        .build()
        .manifest
}

#[must_use]
pub fn scheduled_delivery_runner(bridge: ScheduledAgentDeliveryBridge) -> Box<dyn Runner> {
    Box::new(ScheduledDeliveryRunner {
        descriptor: scheduled_delivery_descriptor(),
        bridge,
    })
}

struct ScheduledDeliveryRunner {
    descriptor: mutsuki_runtime_contracts::RunnerDescriptor,
    bridge: ScheduledAgentDeliveryBridge,
}

impl Runner for ScheduledDeliveryRunner {
    fn descriptor(&self) -> &mutsuki_runtime_contracts::RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, |task| {
            let request: ScheduledDeliveryRequest = serde_json::from_value(task.payload.to_value())
                .map_err(|error| runtime_error(task, error))?;
            let receipt = self
                .bridge
                .deliver(request.result, request.now_unix_ms)
                .map_err(|error| runtime_error(task, error))?;
            let mut completed = RunnerResult::completed(task.task_id.clone());
            completed.output =
                Some(serde_json::to_value(receipt).map_err(|error| runtime_error(task, error))?);
            Ok(completed)
        })
    }
}

fn scheduled_delivery_descriptor() -> mutsuki_runtime_contracts::RunnerDescriptor {
    RunnerDescriptorBuilder::new(
        BOT_SCHEDULED_DELIVERY_RUNNER_ID,
        BOT_SCHEDULED_DELIVERY_PLUGIN_ID,
    )
    .accepted_protocol(BOT_SCHEDULED_DELIVERY_PROTOCOL_ID)
    .execution_class(ExecutionClass::Blocking)
    .build()
}

fn validate_request(request: &BotActiveDeliveryRequest) -> Result<(), DeliveryError> {
    if request.delivery_id.trim().is_empty()
        || request.idempotency_key.trim().is_empty()
        || request.content.segments.is_empty()
        || request.conversation.target().is_none()
        || request.policy.max_attempts == 0
        || request.policy.initial_backoff_ms == 0
        || request.policy.max_backoff_ms < request.policy.initial_backoff_ms
    {
        return Err(DeliveryError::InvalidRequest);
    }
    for segment in &request.content.segments {
        let resource = match segment {
            MessageSegment::Image { resource }
            | MessageSegment::Audio { resource }
            | MessageSegment::Video { resource }
            | MessageSegment::File { resource, .. } => Some(resource),
            _ => None,
        };
        if resource.is_some_and(|resource| {
            resource.provider_id.trim().is_empty()
                || resource.schema.trim().is_empty()
                || resource.size_hint.is_none()
                || resource.content_hash.as_deref().is_none_or(str::is_empty)
        }) {
            return Err(DeliveryError::InvalidRequest);
        }
    }
    Ok(())
}

pub const BOT_DELIVERY_PLUGIN_ID: &str = "mutsuki.plugin.bot.delivery";
pub const BOT_DELIVERY_RUNNER_ID: &str = "mutsuki.bot.delivery.runner";

#[must_use]
pub fn bot_delivery_manifest() -> PluginManifest {
    PluginBuilder::new(BOT_DELIVERY_PLUGIN_ID)
        .runner_descriptor(delivery_descriptor())
        .protocol_handler(
            ProtocolDescriptorBuilder::new(BOT_ACTIVE_DELIVERY_PROTOCOL_ID).build(),
            BOT_DELIVERY_RUNNER_ID,
            "bot-delivery",
        )
        .build()
        .manifest
}

#[must_use]
pub fn delivery_runner(service: ActiveDeliveryService) -> Box<dyn Runner> {
    Box::new(DeliveryRunner {
        descriptor: delivery_descriptor(),
        service,
    })
}

struct DeliveryRunner {
    descriptor: mutsuki_runtime_contracts::RunnerDescriptor,
    service: ActiveDeliveryService,
}

impl Runner for DeliveryRunner {
    fn descriptor(&self) -> &mutsuki_runtime_contracts::RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, |task| delivery_result(&self.service, task))
    }
}

fn delivery_descriptor() -> mutsuki_runtime_contracts::RunnerDescriptor {
    RunnerDescriptorBuilder::new(BOT_DELIVERY_RUNNER_ID, BOT_DELIVERY_PLUGIN_ID)
        .accepted_protocol(BOT_ACTIVE_DELIVERY_PROTOCOL_ID)
        .execution_class(ExecutionClass::Blocking)
        .build()
}

fn delivery_result(
    service: &ActiveDeliveryService,
    task: &Task,
) -> Result<RunnerResult, mutsuki_runtime_contracts::RuntimeError> {
    let command: BotActiveDeliveryCommand = serde_json::from_value(task.payload.to_value())
        .map_err(|error| runtime_error(task, error))?;
    let output = match command {
        BotActiveDeliveryCommand::Submit {
            request,
            now_unix_ms,
        } => serde_json::to_value(
            service
                .submit(&request, now_unix_ms)
                .map_err(|error| runtime_error(task, error))?,
        ),
        BotActiveDeliveryCommand::ResumeDue { now_unix_ms } => serde_json::to_value(
            service
                .resume_due(now_unix_ms)
                .map_err(|error| runtime_error(task, error))?,
        ),
        BotActiveDeliveryCommand::Inspect { delivery_id } => serde_json::to_value(
            service
                .inspect(&delivery_id)
                .map_err(|error| runtime_error(task, error))?,
        ),
        BotActiveDeliveryCommand::Retry {
            delivery_id,
            now_unix_ms,
        } => serde_json::to_value(
            service
                .retry(&delivery_id, now_unix_ms)
                .map_err(|error| runtime_error(task, error))?,
        ),
        BotActiveDeliveryCommand::Cancel { delivery_id } => serde_json::to_value(
            service
                .cancel(&delivery_id)
                .map_err(|error| runtime_error(task, error))?,
        ),
    }
    .map_err(|error| runtime_error(task, error))?;
    let mut result = RunnerResult::completed(task.task_id.clone());
    result.output = Some(output);
    Ok(result)
}

fn runtime_error(
    task: &Task,
    error: impl std::fmt::Display,
) -> mutsuki_runtime_contracts::RuntimeError {
    mutsuki_runtime_contracts::RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        BOT_DELIVERY_PLUGIN_ID,
        format!("{}.{}", task.task_id, error),
    )
}

fn receipt(
    request: &BotActiveDeliveryRequest,
    status: DeliveryStatus,
    attempt_count: u32,
    platform_message_ids: Vec<String>,
    part_receipts: Vec<BotDeliveryPartReceipt>,
    delivered_at_unix_ms: Option<u64>,
    error_code: Option<String>,
) -> BotDeliveryReceipt {
    BotDeliveryReceipt {
        delivery_id: request.delivery_id.clone(),
        idempotency_key: request.idempotency_key.clone(),
        status,
        attempt_count,
        platform_message_ids,
        part_receipts,
        delivered_at_unix_ms,
        error_code,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QqDeliveryFailure {
    pub code: String,
    pub transient: bool,
    pub retry_after_ms: Option<u64>,
    pub sent_message_ids: Vec<String>,
    pub part_receipts: Vec<BotDeliveryPartReceipt>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QqDeliverySuccess {
    pub platform_message_ids: Vec<String>,
    pub part_receipts: Vec<BotDeliveryPartReceipt>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DeliveryError {
    #[error("delivery request is invalid")]
    InvalidRequest,
    #[error("conversation policy denies active delivery")]
    PolicyDenied,
    #[error("delivery repository failed: {0}")]
    Repository(String),
    #[error("delivery was not found")]
    NotFound,
    #[error("delivery id and idempotency key identify different requests")]
    Conflict,
    #[error("delivery operation is invalid for its current state")]
    InvalidState,
    #[error("scheduled Agent run did not succeed")]
    ScheduledRunNotSucceeded,
    #[error("scheduled delivery target must be an opaque Bot conversation binding")]
    InvalidScheduleTarget,
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, VecDeque};
    use std::sync::Mutex;

    use mutsuki_agent_contracts::ScheduleTargetRef;
    use mutsuki_bot_protocol::{
        BotConversationKind, DeliveryPolicy, MessageSegment, QQ_CONVERSATION_REF_VERSION,
    };

    use super::*;

    #[derive(Default)]
    struct Repository {
        requests: Mutex<BTreeMap<String, BotActiveDeliveryRequest>>,
        keys: Mutex<BTreeMap<String, String>>,
        attempts: Mutex<Vec<BotDeliveryAttempt>>,
        receipts: Mutex<BTreeMap<String, BotDeliveryReceipt>>,
    }

    impl DeliveryRepository for Repository {
        fn reserve(
            &self,
            request: &BotActiveDeliveryRequest,
        ) -> Result<Option<BotDeliveryReceipt>, DeliveryError> {
            if let Some(delivery_id) = self.keys.lock().unwrap().get(&request.idempotency_key) {
                return Ok(Some(
                    self.receipts
                        .lock()
                        .unwrap()
                        .get(delivery_id)
                        .cloned()
                        .unwrap_or_else(|| BotDeliveryReceipt {
                            delivery_id: delivery_id.clone(),
                            idempotency_key: request.idempotency_key.clone(),
                            status: DeliveryStatus::Pending,
                            attempt_count: 0,
                            platform_message_ids: Vec::new(),
                            part_receipts: Vec::new(),
                            delivered_at_unix_ms: None,
                            error_code: None,
                        }),
                ));
            }
            if self
                .requests
                .lock()
                .unwrap()
                .contains_key(&request.delivery_id)
            {
                return Err(DeliveryError::Conflict);
            }
            self.keys
                .lock()
                .unwrap()
                .insert(request.idempotency_key.clone(), request.delivery_id.clone());
            self.requests
                .lock()
                .unwrap()
                .insert(request.delivery_id.clone(), request.clone());
            Ok(None)
        }

        fn request(&self, delivery_id: &str) -> Result<BotActiveDeliveryRequest, DeliveryError> {
            self.requests
                .lock()
                .unwrap()
                .get(delivery_id)
                .cloned()
                .ok_or(DeliveryError::NotFound)
        }

        fn receipt(&self, delivery_id: &str) -> Result<BotDeliveryReceipt, DeliveryError> {
            self.receipts
                .lock()
                .unwrap()
                .get(delivery_id)
                .cloned()
                .ok_or(DeliveryError::NotFound)
        }

        fn attempts(&self, delivery_id: &str) -> Result<Vec<BotDeliveryAttempt>, DeliveryError> {
            Ok(self
                .attempts
                .lock()
                .unwrap()
                .iter()
                .filter(|attempt| attempt.delivery_id == delivery_id)
                .cloned()
                .collect())
        }

        fn save_attempt(&self, attempt: BotDeliveryAttempt) -> Result<(), DeliveryError> {
            self.attempts.lock().unwrap().push(attempt);
            Ok(())
        }

        fn save_receipt(&self, receipt: BotDeliveryReceipt) -> Result<(), DeliveryError> {
            self.receipts
                .lock()
                .unwrap()
                .insert(receipt.delivery_id.clone(), receipt);
            Ok(())
        }

        fn due_delivery_ids(&self, now_unix_ms: u64) -> Result<Vec<String>, DeliveryError> {
            Ok(self
                .attempts
                .lock()
                .unwrap()
                .iter()
                .filter(|attempt| {
                    attempt.status == DeliveryStatus::RetryScheduled
                        && attempt
                            .retry_at_unix_ms
                            .is_some_and(|retry_at| retry_at <= now_unix_ms)
                })
                .map(|attempt| attempt.delivery_id.clone())
                .collect())
        }
    }

    struct Gateway {
        results: Mutex<VecDeque<Result<QqDeliverySuccess, QqDeliveryFailure>>>,
        calls: Mutex<u32>,
    }

    struct AllowDelivery;

    impl DeliveryPolicyResolver for AllowDelivery {
        fn active_delivery_allowed(
            &self,
            _conversation: &QqConversationRef,
        ) -> Result<bool, DeliveryError> {
            Ok(true)
        }
    }

    impl QqDeliveryGateway for Gateway {
        fn send(
            &self,
            _conversation: &QqConversationRef,
            _content: &BotDeliveryContent,
        ) -> Result<QqDeliverySuccess, QqDeliveryFailure> {
            *self.calls.lock().unwrap() += 1;
            self.results.lock().unwrap().pop_front().unwrap()
        }
    }

    #[test]
    fn duplicate_key_sends_once_and_transient_attempt_resumes_after_restart() {
        let repository = Arc::new(Repository::default());
        let gateway = Arc::new(Gateway {
            results: Mutex::new(VecDeque::from([
                Err(QqDeliveryFailure {
                    code: "qq.rate_limited".into(),
                    transient: true,
                    retry_after_ms: Some(50),
                    sent_message_ids: Vec::new(),
                    part_receipts: Vec::new(),
                }),
                Ok(QqDeliverySuccess {
                    platform_message_ids: vec!["message".into()],
                    part_receipts: vec![BotDeliveryPartReceipt {
                        part_index: 0,
                        status: mutsuki_bot_protocol::DeliveryPartStatus::Succeeded,
                        platform_message_id: Some("message".into()),
                        error_code: None,
                    }],
                }),
            ])),
            calls: Mutex::new(0),
        });
        let service = ActiveDeliveryService::new(
            repository.clone(),
            gateway.clone(),
            Arc::new(AllowDelivery),
        );
        let request = request();
        let retry = service.submit(&request, 100).unwrap();
        assert_eq!(retry.status, DeliveryStatus::RetryScheduled);
        assert!(service.resume_due(149).unwrap().is_empty());

        let restarted =
            ActiveDeliveryService::new(repository, gateway.clone(), Arc::new(AllowDelivery));
        let receipts = restarted.resume_due(150).unwrap();
        assert_eq!(receipts[0].status, DeliveryStatus::Succeeded);
        let duplicate = restarted.submit(&request, 200).unwrap();
        assert_eq!(duplicate.status, DeliveryStatus::Succeeded);
        assert_eq!(*gateway.calls.lock().unwrap(), 2);
    }

    #[test]
    fn in_flight_duplicate_returns_the_original_delivery_identity() {
        let repository = Repository::default();
        let original = request();
        assert!(repository.reserve(&original).unwrap().is_none());
        let mut duplicate = original.clone();
        duplicate.delivery_id = "duplicate-delivery".into();

        let receipt = repository.reserve(&duplicate).unwrap().unwrap();

        assert_eq!(receipt.delivery_id, original.delivery_id);
        assert_eq!(receipt.status, DeliveryStatus::Pending);
    }

    #[test]
    fn management_operations_expose_history_cancel_due_work_and_retry_failure() {
        let repository = Arc::new(Repository::default());
        let gateway = Arc::new(Gateway {
            results: Mutex::new(VecDeque::from([
                Err(QqDeliveryFailure {
                    code: "qq.rate_limited".into(),
                    transient: true,
                    retry_after_ms: Some(50),
                    sent_message_ids: Vec::new(),
                    part_receipts: Vec::new(),
                }),
                Err(QqDeliveryFailure {
                    code: "qq.rejected".into(),
                    transient: false,
                    retry_after_ms: None,
                    sent_message_ids: Vec::new(),
                    part_receipts: Vec::new(),
                }),
                Ok(QqDeliverySuccess {
                    platform_message_ids: vec!["manual-retry".into()],
                    part_receipts: Vec::new(),
                }),
            ])),
            calls: Mutex::new(0),
        });
        let service = ActiveDeliveryService::new(
            repository.clone(),
            gateway.clone(),
            Arc::new(AllowDelivery),
        );

        assert_eq!(
            service.submit(&request(), 100).unwrap().status,
            DeliveryStatus::RetryScheduled
        );
        let (receipt, attempts) = service.inspect("delivery").unwrap();
        assert_eq!(receipt.attempt_count, 1);
        assert_eq!(attempts.len(), 2);
        assert_eq!(
            service.cancel("delivery").unwrap().status,
            DeliveryStatus::Cancelled
        );
        assert!(service.resume_due(150).unwrap().is_empty());
        assert_eq!(
            service.retry("delivery", 151),
            Err(DeliveryError::InvalidState)
        );

        let mut second = request();
        second.delivery_id = "failed-delivery".into();
        second.idempotency_key = "failed-key".into();
        assert_eq!(
            service.submit(&second, 200).unwrap().status,
            DeliveryStatus::PermanentlyFailed
        );
        let retried = service.retry("failed-delivery", 201).unwrap();
        assert_eq!(retried.status, DeliveryStatus::Succeeded);
        assert_eq!(retried.attempt_count, 2);
        assert_eq!(*gateway.calls.lock().unwrap(), 3);
    }

    struct ScheduledTarget;

    impl ScheduledDeliveryTargetResolver for ScheduledTarget {
        fn resolve_binding(&self, binding_id: &str) -> Result<QqConversationRef, DeliveryError> {
            assert_eq!(binding_id, "daily-report-target");
            Ok(request().conversation)
        }
    }

    impl ScheduledDeliveryPolicyProvider for AllowDelivery {
        fn delivery_policy(
            &self,
            _conversation: &QqConversationRef,
        ) -> Result<DeliveryPolicy, DeliveryError> {
            Ok(request().policy)
        }
    }

    #[test]
    fn scheduled_agent_result_resolves_persisted_binding_and_is_idempotent() {
        let repository = Arc::new(Repository::default());
        let gateway = Arc::new(Gateway {
            results: Mutex::new(VecDeque::from([Ok(QqDeliverySuccess {
                platform_message_ids: vec!["scheduled-message".into()],
                part_receipts: Vec::new(),
            })])),
            calls: Mutex::new(0),
        });
        let active =
            ActiveDeliveryService::new(repository, gateway.clone(), Arc::new(AllowDelivery));
        let bridge = ScheduledAgentDeliveryBridge::new(
            active,
            Arc::new(ScheduledTarget),
            Arc::new(AllowDelivery),
        );
        let result = ScheduledRunResult {
            schedule_id: "daily-report".into(),
            execution_id: "execution-1".into(),
            status: ScheduleExecutionStatus::Succeeded,
            summary: "report ready".into(),
            output_ref: None,
            target: Some(ScheduleTargetRef {
                target_id: "daily-report-target".into(),
                kind: SCHEDULE_TARGET_KIND_BOT_CONVERSATION_BINDING.into(),
                metadata: serde_json::json!({"user_id": "must-not-be-trusted"}),
            }),
        };

        let first = bridge.deliver(result.clone(), 100).unwrap();
        let duplicate = bridge.deliver(result, 101).unwrap();

        assert_eq!(first.delivery_id, "agent-scheduled:execution-1");
        assert_eq!(duplicate, first);
        assert_eq!(*gateway.calls.lock().unwrap(), 1);
    }

    #[test]
    fn scheduled_result_rejects_raw_platform_target_kind() {
        let repository = Arc::new(Repository::default());
        let gateway = Arc::new(Gateway {
            results: Mutex::new(VecDeque::new()),
            calls: Mutex::new(0),
        });
        let bridge = ScheduledAgentDeliveryBridge::new(
            ActiveDeliveryService::new(repository, gateway, Arc::new(AllowDelivery)),
            Arc::new(ScheduledTarget),
            Arc::new(AllowDelivery),
        );
        let result = ScheduledRunResult {
            schedule_id: "schedule".into(),
            execution_id: "execution".into(),
            status: ScheduleExecutionStatus::Succeeded,
            summary: "summary".into(),
            output_ref: None,
            target: Some(ScheduleTargetRef {
                target_id: "raw-openid".into(),
                kind: "qq.user_id".into(),
                metadata: serde_json::Value::Null,
            }),
        };

        assert_eq!(
            bridge.deliver(result, 100),
            Err(DeliveryError::InvalidScheduleTarget)
        );
    }

    fn request() -> BotActiveDeliveryRequest {
        BotActiveDeliveryRequest {
            delivery_id: "delivery".into(),
            idempotency_key: "key".into(),
            conversation: QqConversationRef {
                version: QQ_CONVERSATION_REF_VERSION,
                account_id: "main".into(),
                kind: BotConversationKind::Private,
                user_id: Some("user".into()),
                group_id: None,
                guild_id: None,
                channel_id: None,
                thread_id: None,
            },
            content: BotDeliveryContent {
                segments: vec![MessageSegment::text("hello")],
                summary: None,
            },
            policy: DeliveryPolicy {
                max_attempts: 3,
                initial_backoff_ms: 10,
                max_backoff_ms: 1_000,
                not_before_unix_ms: None,
                expires_at_unix_ms: None,
            },
            dry_run: false,
            source_execution_id: None,
        }
    }
}
