// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::default_trait_access,
    clippy::doc_markdown,
    clippy::map_unwrap_or,
    clippy::missing_panics_doc,
    clippy::needless_pass_by_value,
    clippy::redundant_closure_for_method_calls
)]

use async_trait::async_trait;
use mutsuki_agent_contracts::{ScheduleExecutionStatus, ScheduledRunResult};
use mutsuki_bot_protocol::{
    BOT_MESSAGE_SEND_PROTOCOL_ID, BotActiveDeliveryRequest, BotDeliveryAttempt, BotDeliveryContent,
    BotDeliveryPartReceipt, BotDeliveryReceipt, BotMessage, BotReplyDeliveryPart,
    BotReplyDeliveryReceipt, BotReplyDeliveryRequest, BotTarget, DeliveryPartStatus,
    DeliveryPolicy, DeliveryStatus, MessageSegment, QqConversationRef,
};
use mutsuki_runtime_contracts::{ScalarValue, TaskOutcome};
use mutsuki_runtime_core::RuntimeFailure;
use mutsuki_runtime_sdk::AsyncRunnerContext;
use std::sync::Arc;
use thiserror::Error;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ScheduledDeliveryRequest {
    pub result: ScheduledRunResult,
    pub now_unix_ms: u64,
}

#[async_trait]
pub trait DeliveryRepository: Send + Sync {
    async fn reserve(
        &self,
        request: &BotActiveDeliveryRequest,
    ) -> Result<Option<BotDeliveryReceipt>, DeliveryError>;
    async fn request(&self, delivery_id: &str) -> Result<BotActiveDeliveryRequest, DeliveryError>;
    async fn receipt(&self, delivery_id: &str) -> Result<BotDeliveryReceipt, DeliveryError>;
    async fn attempts(&self, delivery_id: &str) -> Result<Vec<BotDeliveryAttempt>, DeliveryError>;
    async fn save_outcome(
        &self,
        attempt: BotDeliveryAttempt,
        receipt: BotDeliveryReceipt,
    ) -> Result<(), DeliveryError>;
    async fn save_receipt(&self, receipt: BotDeliveryReceipt) -> Result<(), DeliveryError>;
    /// Claims due deliveries with a send lease: Pending, due RetryScheduled.
    /// Expired Sending leases become ReconcileRequired and are not returned.
    async fn claim_due_delivery_ids(&self, now_unix_ms: u64) -> Result<Vec<String>, DeliveryError>;
    /// CAS-claims a single delivery for an outbound send attempt.
    async fn begin_send(
        &self,
        delivery_id: &str,
        attempt: BotDeliveryAttempt,
        now_unix_ms: u64,
        lease_ms: u64,
    ) -> Result<BotDeliveryReceipt, DeliveryError>;
}

#[async_trait]
pub trait ReplyDeliveryRepository: DeliveryRepository {
    /// Atomically reserves a reply bundle and every durable part receipt.
    async fn reserve_reply(
        &self,
        request: &BotReplyDeliveryRequest,
    ) -> Result<Option<BotReplyDeliveryReceipt>, DeliveryError>;
    async fn reply_receipt(&self, reply_id: &str)
    -> Result<BotReplyDeliveryReceipt, DeliveryError>;
    /// CAS-claims only reply parts, excluding ordinary active deliveries.
    async fn claim_due_reply_part_id(
        &self,
        now_unix_ms: u64,
    ) -> Result<Option<String>, DeliveryError>;
    async fn is_reply_part(&self, delivery_id: &str) -> Result<bool, DeliveryError>;
}

pub const DELIVERY_SEND_LEASE_MS: u64 = 30_000;

pub trait DeliveryGateway: Send + Sync {
    fn send(
        &self,
        target: &BotTarget,
        content: &BotDeliveryContent,
    ) -> Result<DeliverySuccess, DeliveryFailure>;
}

pub trait DeliveryPolicyResolver: Send + Sync {
    fn active_delivery_allowed(
        &self,
        conversation: &QqConversationRef,
    ) -> Result<bool, DeliveryError>;
}

enum AttemptStart {
    Finished(BotDeliveryReceipt),
    Sending(BotDeliveryReceipt),
}

#[derive(Clone)]
struct DeliveryAttemptService {
    repository: Arc<dyn DeliveryRepository>,
}

impl DeliveryAttemptService {
    fn new(repository: Arc<dyn DeliveryRepository>) -> Self {
        Self { repository }
    }

    async fn begin(
        &self,
        request: &BotActiveDeliveryRequest,
        now_unix_ms: u64,
        attempt_number: u32,
    ) -> Result<AttemptStart, DeliveryError> {
        let mut receipt = self.repository.receipt(&request.delivery_id).await?;
        if let Some(not_before) = request.policy.not_before_unix_ms
            && now_unix_ms < not_before
        {
            receipt.status = DeliveryStatus::RetryScheduled;
            receipt.attempt_count = attempt_number.saturating_sub(1);
            receipt.error_code = None;
            receipt.lease_expires_at_unix_ms = None;
            self.save(receipt.clone(), now_unix_ms, Some(not_before), None)
                .await?;
            return Ok(AttemptStart::Finished(receipt));
        }
        if request
            .policy
            .expires_at_unix_ms
            .is_some_and(|expires| now_unix_ms >= expires)
        {
            return self
                .permanent(
                    receipt,
                    attempt_number,
                    now_unix_ms,
                    "delivery.expired",
                    Vec::new(),
                    Vec::new(),
                )
                .await
                .map(AttemptStart::Finished);
        }
        let attempt = BotDeliveryAttempt {
            delivery_id: request.delivery_id.clone(),
            attempt: attempt_number,
            status: DeliveryStatus::Sending,
            started_at_unix_ms: now_unix_ms,
            retry_at_unix_ms: None,
            error_code: None,
        };
        self.repository
            .begin_send(
                &request.delivery_id,
                attempt,
                now_unix_ms,
                DELIVERY_SEND_LEASE_MS,
            )
            .await
            .map(AttemptStart::Sending)
    }

    async fn fail(
        &self,
        request: &BotActiveDeliveryRequest,
        mut receipt: BotDeliveryReceipt,
        attempt_number: u32,
        now_unix_ms: u64,
        failure: DeliveryFailure,
    ) -> Result<BotDeliveryReceipt, DeliveryError> {
        if failure.transient && attempt_number < request.policy.max_attempts.max(1) {
            let exponential = request
                .policy
                .initial_backoff_ms
                .saturating_mul(1_u64 << attempt_number.saturating_sub(1).min(20));
            let retry_at = now_unix_ms.saturating_add(
                failure
                    .retry_after_ms
                    .unwrap_or(exponential)
                    .min(request.policy.max_backoff_ms),
            );
            receipt.status = DeliveryStatus::RetryScheduled;
            receipt.attempt_count = attempt_number;
            receipt.platform_message_ids = failure.sent_message_ids;
            receipt.part_receipts = failure.part_receipts;
            receipt.error_code = Some(failure.code.clone());
            receipt.lease_expires_at_unix_ms = None;
            self.save(
                receipt.clone(),
                now_unix_ms,
                Some(retry_at),
                Some(failure.code),
            )
            .await?;
            Ok(receipt)
        } else {
            self.permanent(
                receipt,
                attempt_number,
                now_unix_ms,
                &failure.code,
                failure.sent_message_ids,
                failure.part_receipts,
            )
            .await
        }
    }

    async fn succeed(
        &self,
        mut receipt: BotDeliveryReceipt,
        attempt_number: u32,
        now_unix_ms: u64,
        platform_message_ids: Vec<String>,
        part_receipts: Vec<BotDeliveryPartReceipt>,
    ) -> Result<BotDeliveryReceipt, DeliveryError> {
        receipt.status = DeliveryStatus::Succeeded;
        receipt.attempt_count = attempt_number;
        receipt.platform_message_ids = platform_message_ids;
        receipt.part_receipts = part_receipts;
        receipt.delivered_at_unix_ms = Some(now_unix_ms);
        receipt.error_code = None;
        receipt.lease_expires_at_unix_ms = None;
        self.save(receipt.clone(), now_unix_ms, None, None).await?;
        Ok(receipt)
    }

    async fn permanent(
        &self,
        mut receipt: BotDeliveryReceipt,
        attempt_number: u32,
        now_unix_ms: u64,
        code: &str,
        sent_message_ids: Vec<String>,
        part_receipts: Vec<BotDeliveryPartReceipt>,
    ) -> Result<BotDeliveryReceipt, DeliveryError> {
        receipt.status = DeliveryStatus::PermanentlyFailed;
        receipt.attempt_count = attempt_number;
        receipt.platform_message_ids = sent_message_ids;
        receipt.part_receipts = part_receipts;
        receipt.error_code = Some(code.into());
        receipt.lease_expires_at_unix_ms = None;
        self.save(receipt.clone(), now_unix_ms, None, Some(code.into()))
            .await?;
        Ok(receipt)
    }

    async fn reconcile(
        &self,
        mut receipt: BotDeliveryReceipt,
        attempt_number: u32,
        now_unix_ms: u64,
        code: &str,
    ) -> Result<BotDeliveryReceipt, DeliveryError> {
        receipt.status = DeliveryStatus::ReconcileRequired;
        receipt.attempt_count = attempt_number;
        receipt.error_code = Some(code.into());
        receipt.lease_expires_at_unix_ms = None;
        self.save(receipt.clone(), now_unix_ms, None, Some(code.into()))
            .await?;
        Ok(receipt)
    }

    async fn save(
        &self,
        receipt: BotDeliveryReceipt,
        started_at_unix_ms: u64,
        retry_at_unix_ms: Option<u64>,
        error_code: Option<String>,
    ) -> Result<(), DeliveryError> {
        self.repository
            .save_outcome(
                BotDeliveryAttempt {
                    delivery_id: receipt.delivery_id.clone(),
                    attempt: receipt.attempt_count,
                    status: receipt.status,
                    started_at_unix_ms,
                    retry_at_unix_ms,
                    error_code,
                },
                receipt,
            )
            .await
    }
}

#[derive(Clone)]
pub struct ActiveDeliveryService {
    attempts: DeliveryAttemptService,
    gateway: Arc<dyn DeliveryGateway>,
    policy: Arc<dyn DeliveryPolicyResolver>,
}

impl ActiveDeliveryService {
    pub fn new(
        repository: Arc<dyn DeliveryRepository>,
        gateway: Arc<dyn DeliveryGateway>,
        policy: Arc<dyn DeliveryPolicyResolver>,
    ) -> Self {
        Self {
            attempts: DeliveryAttemptService::new(repository),
            gateway,
            policy,
        }
    }

    /// Reserves and attempts one idempotent active delivery.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid input, policy denial, or repository failure.
    pub async fn submit(
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
        if let Some(existing) = self.attempts.repository.reserve(request).await? {
            return Ok(existing);
        }
        self.attempt(request, now_unix_ms, 1).await
    }

    /// Claims and resumes every delivery due at the supplied timestamp.
    ///
    /// # Errors
    ///
    /// Returns an error when claiming, loading, or attempting a due delivery fails.
    pub async fn resume_due(
        &self,
        now_unix_ms: u64,
    ) -> Result<Vec<BotDeliveryReceipt>, DeliveryError> {
        let delivery_ids = self
            .attempts
            .repository
            .claim_due_delivery_ids(now_unix_ms)
            .await?;
        let mut receipts = Vec::with_capacity(delivery_ids.len());
        for delivery_id in delivery_ids {
            let request = self.attempts.repository.request(&delivery_id).await?;
            let attempt = self.next_attempt(&delivery_id).await?;
            receipts.push(self.attempt(&request, now_unix_ms, attempt).await?);
        }
        Ok(receipts)
    }

    /// Reads the current receipt and complete attempt history.
    ///
    /// # Errors
    ///
    /// Returns an error when the delivery does not exist or repository access fails.
    pub async fn inspect(
        &self,
        delivery_id: &str,
    ) -> Result<(BotDeliveryReceipt, Vec<BotDeliveryAttempt>), DeliveryError> {
        Ok((
            self.attempts.repository.receipt(delivery_id).await?,
            self.attempts.repository.attempts(delivery_id).await?,
        ))
    }

    /// Builds a dry-run receipt without sending or mutating delivery state.
    ///
    /// # Errors
    ///
    /// Returns an error when the stored request is unavailable or invalid.
    pub async fn preview(&self, delivery_id: &str) -> Result<BotDeliveryReceipt, DeliveryError> {
        let mut request = self.attempts.repository.request(delivery_id).await?;
        request.dry_run = true;
        self.submit(&request, 0).await
    }

    /// Retries a retryable, permanently failed, or reconcile-required delivery under policy.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid state, policy denial, or repository failure.
    pub async fn retry(
        &self,
        delivery_id: &str,
        now_unix_ms: u64,
    ) -> Result<BotDeliveryReceipt, DeliveryError> {
        let current = self.attempts.repository.receipt(delivery_id).await?;
        if !matches!(
            current.status,
            DeliveryStatus::RetryScheduled
                | DeliveryStatus::PermanentlyFailed
                | DeliveryStatus::ReconcileRequired
        ) {
            return Err(DeliveryError::InvalidState);
        }
        let request = self.attempts.repository.request(delivery_id).await?;
        if !self.policy.active_delivery_allowed(&request.conversation)? {
            return Err(DeliveryError::PolicyDenied);
        }
        self.attempt(
            &request,
            now_unix_ms,
            current.attempt_count.saturating_add(1),
        )
        .await
    }

    /// Cancels a delivery that is waiting for retry or manual reconcile.
    ///
    /// # Errors
    ///
    /// Returns an error when the delivery is not cancellable or persistence fails.
    pub async fn cancel(&self, delivery_id: &str) -> Result<BotDeliveryReceipt, DeliveryError> {
        let mut current = self.attempts.repository.receipt(delivery_id).await?;
        if !matches!(
            current.status,
            DeliveryStatus::RetryScheduled | DeliveryStatus::ReconcileRequired
        ) {
            return Err(DeliveryError::InvalidState);
        }
        current.status = DeliveryStatus::Cancelled;
        current.error_code = None;
        current.lease_expires_at_unix_ms = None;
        self.attempts
            .repository
            .save_receipt(current.clone())
            .await?;
        Ok(current)
    }

    async fn next_attempt(&self, delivery_id: &str) -> Result<u32, DeliveryError> {
        Ok(self
            .attempts
            .repository
            .receipt(delivery_id)
            .await?
            .attempt_count
            .saturating_add(1))
    }

    async fn attempt(
        &self,
        request: &BotActiveDeliveryRequest,
        now_unix_ms: u64,
        attempt_number: u32,
    ) -> Result<BotDeliveryReceipt, DeliveryError> {
        let receipt = match self
            .attempts
            .begin(request, now_unix_ms, attempt_number)
            .await?
        {
            AttemptStart::Finished(receipt) => return Ok(receipt),
            AttemptStart::Sending(receipt) => receipt,
        };
        let target = request
            .conversation
            .target()
            .ok_or(DeliveryError::InvalidRequest)?;
        match self.gateway.send(&target, &request.content) {
            Ok(success) => {
                self.attempts
                    .succeed(
                        receipt,
                        attempt_number,
                        now_unix_ms,
                        success.platform_message_ids,
                        success.part_receipts,
                    )
                    .await
            }
            Err(failure) => {
                self.attempts
                    .fail(request, receipt, attempt_number, now_unix_ms, failure)
                    .await
            }
        }
    }
}

#[derive(Clone)]
pub struct ReplyDeliveryService {
    attempts: DeliveryAttemptService,
    repository: Arc<dyn ReplyDeliveryRepository>,
}

impl ReplyDeliveryService {
    #[must_use]
    pub fn new(repository: Arc<dyn ReplyDeliveryRepository>) -> Self {
        Self {
            attempts: DeliveryAttemptService::new(repository.clone()),
            repository,
        }
    }

    /// Reserves a reply bundle without sending.
    ///
    /// # Errors
    ///
    /// Returns an error when the request is invalid or persistence fails.
    pub async fn reserve(
        &self,
        request: &BotReplyDeliveryRequest,
    ) -> Result<BotReplyDeliveryReceipt, DeliveryError> {
        validate_reply_request(request)?;
        if let Some(existing) = self.repository.reserve_reply(request).await? {
            return Ok(existing);
        }
        self.receipt_for(request).await
    }

    /// Submits a reply bundle and sends due parts.
    ///
    /// # Errors
    ///
    /// Returns an error when reservation, send, or persistence fails.
    pub async fn submit(
        &self,
        ctx: &AsyncRunnerContext,
        request: &BotReplyDeliveryRequest,
        now_unix_ms: u64,
    ) -> Result<BotReplyDeliveryReceipt, DeliveryError> {
        let mut request = request.clone();
        request.occupancy_only = false;
        self.reserve(&request).await?;
        for part in &request.parts {
            let work = reply_part_request(&request, part);
            let current = self.attempts.repository.receipt(&work.delivery_id).await?;
            match current.status {
                DeliveryStatus::Pending => {
                    if work
                        .policy
                        .not_before_unix_ms
                        .is_some_and(|deadline| deadline > now_unix_ms)
                    {
                        continue;
                    }
                    let receipt = self.attempt(ctx, &work, now_unix_ms, 1).await?;
                    if blocks_following_reply_part(receipt.status) {
                        break;
                    }
                }
                status if blocks_following_reply_part(status) => {
                    break;
                }
                _ => {}
            }
        }
        self.receipt_for(&request).await
    }

    /// Inspects a persisted reply bundle.
    ///
    /// # Errors
    ///
    /// Returns an error when the reply is missing or persistence fails.
    pub async fn inspect(&self, reply_id: &str) -> Result<BotReplyDeliveryReceipt, DeliveryError> {
        self.repository.reply_receipt(reply_id).await
    }

    /// Claims and sends the next due reply part.
    ///
    /// # Errors
    ///
    /// Returns an error when claim or send fails.
    pub async fn resume_due(
        &self,
        ctx: &AsyncRunnerContext,
        now_unix_ms: u64,
    ) -> Result<Option<BotDeliveryReceipt>, DeliveryError> {
        let Some(delivery_id) = self.repository.claim_due_reply_part_id(now_unix_ms).await? else {
            return Ok(None);
        };
        let request = self.attempts.repository.request(&delivery_id).await?;
        let attempt = self
            .attempts
            .repository
            .receipt(&delivery_id)
            .await?
            .attempt_count
            .saturating_add(1);
        self.attempt(ctx, &request, now_unix_ms, attempt)
            .await
            .map(Some)
    }

    /// Retries one reply part.
    ///
    /// # Errors
    ///
    /// Returns an error when the part is missing, not retryable, or send fails.
    pub async fn retry_part(
        &self,
        ctx: &AsyncRunnerContext,
        delivery_id: &str,
        now_unix_ms: u64,
    ) -> Result<BotDeliveryReceipt, DeliveryError> {
        if !self.repository.is_reply_part(delivery_id).await? {
            return Err(DeliveryError::NotFound);
        }
        let current = self.attempts.repository.receipt(delivery_id).await?;
        if !matches!(
            current.status,
            DeliveryStatus::RetryScheduled
                | DeliveryStatus::PermanentlyFailed
                | DeliveryStatus::ReconcileRequired
        ) {
            return Err(DeliveryError::InvalidState);
        }
        let request = self.attempts.repository.request(delivery_id).await?;
        self.attempt(
            ctx,
            &request,
            now_unix_ms,
            current.attempt_count.saturating_add(1),
        )
        .await
    }

    /// Cancels one reply part.
    ///
    /// # Errors
    ///
    /// Returns an error when the part is missing or not cancellable.
    pub async fn cancel_part(
        &self,
        delivery_id: &str,
    ) -> Result<BotDeliveryReceipt, DeliveryError> {
        if !self.repository.is_reply_part(delivery_id).await? {
            return Err(DeliveryError::NotFound);
        }
        let mut receipt = self.attempts.repository.receipt(delivery_id).await?;
        if !matches!(
            receipt.status,
            DeliveryStatus::Pending
                | DeliveryStatus::RetryScheduled
                | DeliveryStatus::ReconcileRequired
        ) {
            return Err(DeliveryError::InvalidState);
        }
        receipt.status = DeliveryStatus::Cancelled;
        receipt.error_code = None;
        receipt.lease_expires_at_unix_ms = None;
        self.attempts
            .repository
            .save_receipt(receipt.clone())
            .await?;
        Ok(receipt)
    }

    async fn receipt_for(
        &self,
        request: &BotReplyDeliveryRequest,
    ) -> Result<BotReplyDeliveryReceipt, DeliveryError> {
        let mut part_receipts = Vec::with_capacity(request.parts.len());
        for part in &request.parts {
            part_receipts.push(self.attempts.repository.receipt(&part.part_id).await?);
        }
        Ok(BotReplyDeliveryReceipt {
            reply_id: request.reply_id.clone(),
            idempotency_key: request.idempotency_key.clone(),
            part_receipts,
        })
    }

    async fn attempt(
        &self,
        ctx: &AsyncRunnerContext,
        request: &BotActiveDeliveryRequest,
        now_unix_ms: u64,
        attempt_number: u32,
    ) -> Result<BotDeliveryReceipt, DeliveryError> {
        let receipt = match self
            .attempts
            .begin(request, now_unix_ms, attempt_number)
            .await?
        {
            AttemptStart::Finished(receipt) => return Ok(receipt),
            AttemptStart::Sending(receipt) => receipt,
        };
        let target = request
            .conversation
            .target()
            .ok_or(DeliveryError::InvalidRequest)?;
        let message = BotMessage {
            message_id: None,
            target,
            sender: None,
            segments: request.content.segments.clone(),
            reply_to: request.content.reply_to.clone(),
            time_ms: None,
            ext: Default::default(),
        };
        let outcome = ctx
            .call_raw(
                BOT_MESSAGE_SEND_PROTOCOL_ID,
                serde_json::to_value(message).map_err(|_| DeliveryError::InvalidRequest)?,
            )
            .await;
        self.finish_runtime_outcome(
            request,
            receipt,
            attempt_number,
            now_unix_ms,
            outcome.map(|value| value.into_outcome()),
        )
        .await
    }

    async fn finish_runtime_outcome(
        &self,
        request: &BotActiveDeliveryRequest,
        receipt: BotDeliveryReceipt,
        attempt_number: u32,
        now_unix_ms: u64,
        outcome: Result<TaskOutcome, RuntimeFailure>,
    ) -> Result<BotDeliveryReceipt, DeliveryError> {
        match outcome {
            Ok(TaskOutcome::Completed { output, .. }) => {
                let ids = output
                    .as_ref()
                    .map(platform_message_ids)
                    .unwrap_or_default();
                self.attempts
                    .succeed(
                        receipt,
                        attempt_number,
                        now_unix_ms,
                        ids.clone(),
                        vec![BotDeliveryPartReceipt {
                            part_index: 0,
                            status: DeliveryPartStatus::Succeeded,
                            platform_message_id: ids.first().cloned(),
                            error_code: None,
                        }],
                    )
                    .await
            }
            Ok(TaskOutcome::Failed { error, .. }) => {
                if error.code == mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED
                    && matches!(
                        error.route.as_str(),
                        "host.async_executor.timeout" | "host.async_executor.cancelled"
                    )
                {
                    return self
                        .attempts
                        .reconcile(receipt, attempt_number, now_unix_ms, &error.route)
                        .await;
                }
                let transient = matches!(
                    error.evidence.get("retryable"),
                    Some(ScalarValue::Bool(true))
                );
                let retry_after_ms = match error.evidence.get("retry_after_ms") {
                    Some(ScalarValue::Int(value)) => u64::try_from(*value).ok(),
                    _ => None,
                };
                self.attempts
                    .fail(
                        request,
                        receipt,
                        attempt_number,
                        now_unix_ms,
                        runtime_send_failure(error.code, transient, retry_after_ms),
                    )
                    .await
            }
            Ok(TaskOutcome::Cancelled { .. }) => {
                self.attempts
                    .reconcile(receipt, attempt_number, now_unix_ms, "delivery.cancelled")
                    .await
            }
            Ok(TaskOutcome::Expired { .. }) => {
                self.attempts
                    .reconcile(receipt, attempt_number, now_unix_ms, "delivery.timeout")
                    .await
            }
            Ok(TaskOutcome::DeadLetter { .. }) => {
                self.attempts
                    .permanent(
                        receipt,
                        attempt_number,
                        now_unix_ms,
                        "delivery.dead_letter",
                        Vec::new(),
                        failed_part("delivery.dead_letter"),
                    )
                    .await
            }
            Err(error) => {
                let code = error.error().code.clone();
                self.attempts
                    .fail(
                        request,
                        receipt,
                        attempt_number,
                        now_unix_ms,
                        runtime_send_failure(code, true, None),
                    )
                    .await
            }
        }
    }
}

fn blocks_following_reply_part(status: DeliveryStatus) -> bool {
    matches!(
        status,
        DeliveryStatus::Pending
            | DeliveryStatus::Sending
            | DeliveryStatus::RetryScheduled
            | DeliveryStatus::ReconcileRequired
    )
}

fn failed_part(code: &str) -> Vec<BotDeliveryPartReceipt> {
    vec![BotDeliveryPartReceipt {
        part_index: 0,
        status: DeliveryPartStatus::Failed,
        platform_message_id: None,
        error_code: Some(code.into()),
    }]
}

fn runtime_send_failure(
    code: String,
    transient: bool,
    retry_after_ms: Option<u64>,
) -> DeliveryFailure {
    DeliveryFailure {
        part_receipts: failed_part(&code),
        code,
        transient,
        retry_after_ms,
        sent_message_ids: Vec::new(),
    }
}

#[must_use]
pub fn reply_part_request(
    reply: &BotReplyDeliveryRequest,
    part: &BotReplyDeliveryPart,
) -> BotActiveDeliveryRequest {
    BotActiveDeliveryRequest {
        delivery_id: part.part_id.clone(),
        idempotency_key: part.part_id.clone(),
        conversation: reply.conversation.clone(),
        content: part.content.clone(),
        policy: {
            let mut policy = reply.policy.clone();
            if let Some(not_before_unix_ms) = part.not_before_unix_ms {
                policy.not_before_unix_ms = Some(not_before_unix_ms);
            }
            policy
        },
        dry_run: false,
        source_execution_id: Some(reply.source_turn_id.clone()),
    }
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

fn validate_reply_request(request: &BotReplyDeliveryRequest) -> Result<(), DeliveryError> {
    if request.reply_id.trim().is_empty()
        || request.idempotency_key.trim().is_empty()
        || request.source_event_id.trim().is_empty()
        || request.source_turn_id.trim().is_empty()
        || request.parts.is_empty()
    {
        return Err(DeliveryError::InvalidRequest);
    }
    let mut ids = std::collections::BTreeSet::new();
    for part in &request.parts {
        if part.part_id.trim().is_empty() || !ids.insert(part.part_id.as_str()) {
            return Err(DeliveryError::InvalidRequest);
        }
        validate_request(&reply_part_request(request, part))?;
    }
    Ok(())
}

fn platform_message_ids(value: &serde_json::Value) -> Vec<String> {
    let mut ids = Vec::new();
    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                ids.extend(platform_message_ids(value));
            }
        }
        serde_json::Value::Object(object) => {
            for key in ["id", "message_id"] {
                if let Some(id) = object.get(key).and_then(serde_json::Value::as_str) {
                    ids.push(id.to_owned());
                }
            }
            if let Some(response) = object.get("response") {
                ids.extend(platform_message_ids(response));
            }
        }
        _ => {}
    }
    ids
}

pub const SCHEDULE_TARGET_KIND_BOT_CONVERSATION_BINDING: &str = "mutsuki.bot.conversation_binding";
pub const BOT_SCHEDULED_DELIVERY_PROTOCOL_ID: &str = "mutsuki.bot.delivery/scheduled-result@1";

pub trait ScheduledDeliveryTargetResolver: Send + Sync {
    /// Resolves an owner-persisted opaque target binding.
    ///
    /// # Errors
    ///
    /// Returns an error when the binding is missing, invalid, or unavailable.
    fn resolve_binding(&self, binding_id: &str) -> Result<QqConversationRef, DeliveryError>;
}

pub trait ScheduledDeliveryPolicyProvider: Send + Sync {
    /// Resolves the current delivery policy for a bound conversation.
    ///
    /// # Errors
    ///
    /// Returns an error when policy state cannot be resolved.
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

    /// Converts a successful scheduled Agent result into an active Bot delivery.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsuccessful run, invalid target, or delivery failure.
    pub async fn deliver(
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
                reply_to: None,
            },
            policy,
            dry_run: false,
            source_execution_id: Some(result.execution_id),
        };
        self.delivery.submit(&request, now_unix_ms).await
    }
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
        generation: 0,
        lease_expires_at_unix_ms: None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeliveryFailure {
    pub code: String,
    pub transient: bool,
    pub retry_after_ms: Option<u64>,
    pub sent_message_ids: Vec<String>,
    pub part_receipts: Vec<BotDeliveryPartReceipt>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeliverySuccess {
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

impl DeliveryError {
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidRequest => "delivery.invalid_request",
            Self::PolicyDenied => "delivery.policy_denied",
            Self::Repository(_) => "delivery.repository_failed",
            Self::NotFound => "delivery.not_found",
            Self::Conflict => "delivery.conflict",
            Self::InvalidState => "delivery.invalid_state",
            Self::ScheduledRunNotSucceeded => "delivery.schedule_not_succeeded",
            Self::InvalidScheduleTarget => "delivery.invalid_schedule_target",
        }
    }
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

    #[async_trait]
    impl DeliveryRepository for Repository {
        async fn reserve(
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
                            generation: 0,
                            lease_expires_at_unix_ms: None,
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
            let pending = BotDeliveryReceipt {
                delivery_id: request.delivery_id.clone(),
                idempotency_key: request.idempotency_key.clone(),
                status: DeliveryStatus::Pending,
                attempt_count: 0,
                platform_message_ids: Vec::new(),
                part_receipts: Vec::new(),
                delivered_at_unix_ms: None,
                error_code: None,
                generation: 0,
                lease_expires_at_unix_ms: None,
            };
            self.receipts
                .lock()
                .unwrap()
                .insert(request.delivery_id.clone(), pending);
            Ok(None)
        }

        async fn request(
            &self,
            delivery_id: &str,
        ) -> Result<BotActiveDeliveryRequest, DeliveryError> {
            self.requests
                .lock()
                .unwrap()
                .get(delivery_id)
                .cloned()
                .ok_or(DeliveryError::NotFound)
        }

        async fn receipt(&self, delivery_id: &str) -> Result<BotDeliveryReceipt, DeliveryError> {
            self.receipts
                .lock()
                .unwrap()
                .get(delivery_id)
                .cloned()
                .ok_or(DeliveryError::NotFound)
        }

        async fn attempts(
            &self,
            delivery_id: &str,
        ) -> Result<Vec<BotDeliveryAttempt>, DeliveryError> {
            Ok(self
                .attempts
                .lock()
                .unwrap()
                .iter()
                .filter(|attempt| attempt.delivery_id == delivery_id)
                .cloned()
                .collect())
        }

        async fn save_outcome(
            &self,
            attempt: BotDeliveryAttempt,
            receipt: BotDeliveryReceipt,
        ) -> Result<(), DeliveryError> {
            self.attempts.lock().unwrap().push(attempt);
            self.receipts
                .lock()
                .unwrap()
                .insert(receipt.delivery_id.clone(), receipt);
            Ok(())
        }

        async fn save_receipt(&self, receipt: BotDeliveryReceipt) -> Result<(), DeliveryError> {
            self.receipts
                .lock()
                .unwrap()
                .insert(receipt.delivery_id.clone(), receipt);
            Ok(())
        }

        async fn claim_due_delivery_ids(
            &self,
            now_unix_ms: u64,
        ) -> Result<Vec<String>, DeliveryError> {
            let mut receipts = self.receipts.lock().unwrap();
            let mut claimed = Vec::new();
            let ids = receipts.keys().cloned().collect::<Vec<_>>();
            for delivery_id in ids {
                let Some(receipt) = receipts.get_mut(&delivery_id) else {
                    continue;
                };
                match receipt.status {
                    DeliveryStatus::Sending => {
                        if receipt
                            .lease_expires_at_unix_ms
                            .is_none_or(|expires| expires <= now_unix_ms)
                        {
                            receipt.status = DeliveryStatus::ReconcileRequired;
                            receipt.lease_expires_at_unix_ms = None;
                            receipt.error_code = Some("delivery.reconcile_required".into());
                        }
                    }
                    DeliveryStatus::Pending => {
                        receipt.status = DeliveryStatus::Sending;
                        receipt.generation = receipt.generation.saturating_add(1);
                        receipt.lease_expires_at_unix_ms =
                            Some(now_unix_ms.saturating_add(DELIVERY_SEND_LEASE_MS));
                        claimed.push(delivery_id);
                    }
                    DeliveryStatus::RetryScheduled => {
                        let due = self.attempts.lock().unwrap().iter().any(|attempt| {
                            attempt.delivery_id == delivery_id
                                && attempt.status == DeliveryStatus::RetryScheduled
                                && attempt
                                    .retry_at_unix_ms
                                    .is_some_and(|retry_at| retry_at <= now_unix_ms)
                        });
                        if !due {
                            continue;
                        }
                        receipt.status = DeliveryStatus::Sending;
                        receipt.generation = receipt.generation.saturating_add(1);
                        receipt.lease_expires_at_unix_ms =
                            Some(now_unix_ms.saturating_add(DELIVERY_SEND_LEASE_MS));
                        claimed.push(delivery_id);
                    }
                    _ => {}
                }
            }
            Ok(claimed)
        }

        async fn begin_send(
            &self,
            delivery_id: &str,
            attempt: BotDeliveryAttempt,
            now_unix_ms: u64,
            lease_ms: u64,
        ) -> Result<BotDeliveryReceipt, DeliveryError> {
            let mut receipts = self.receipts.lock().unwrap();
            let receipt = receipts
                .get_mut(delivery_id)
                .ok_or(DeliveryError::NotFound)?;
            let claimable = match receipt.status {
                DeliveryStatus::Pending
                | DeliveryStatus::RetryScheduled
                | DeliveryStatus::PermanentlyFailed
                | DeliveryStatus::ReconcileRequired => true,
                DeliveryStatus::Sending => receipt
                    .lease_expires_at_unix_ms
                    .is_some_and(|expires| expires > now_unix_ms),
                _ => false,
            };
            if !claimable {
                return Err(DeliveryError::InvalidState);
            }
            if !matches!(receipt.status, DeliveryStatus::Sending)
                || receipt
                    .lease_expires_at_unix_ms
                    .is_none_or(|expires| expires <= now_unix_ms)
            {
                receipt.status = DeliveryStatus::Sending;
                receipt.generation = receipt.generation.saturating_add(1);
                receipt.lease_expires_at_unix_ms = Some(now_unix_ms.saturating_add(lease_ms));
                receipt.error_code = None;
            }
            let claimed = receipt.clone();
            drop(receipts);
            self.attempts.lock().unwrap().push(attempt);
            Ok(claimed)
        }
    }

    struct Gateway {
        results: Mutex<VecDeque<Result<DeliverySuccess, DeliveryFailure>>>,
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

    impl DeliveryGateway for Gateway {
        fn send(
            &self,
            _target: &BotTarget,
            _content: &BotDeliveryContent,
        ) -> Result<DeliverySuccess, DeliveryFailure> {
            *self.calls.lock().unwrap() += 1;
            self.results.lock().unwrap().pop_front().unwrap()
        }
    }

    #[test]
    fn pending_resume_expired_sending_reconcile_retry_and_cancel() {
        futures::executor::block_on(async {
            let repository = Arc::new(Repository::default());
            let gateway = Arc::new(Gateway {
                results: Mutex::new(VecDeque::from([
                    Ok(DeliverySuccess {
                        platform_message_ids: vec!["recovered".into()],
                        part_receipts: Vec::new(),
                    }),
                    Ok(DeliverySuccess {
                        platform_message_ids: vec!["reconciled".into()],
                        part_receipts: Vec::new(),
                    }),
                ])),
                calls: Mutex::new(0),
            });
            let request = request();
            assert!(repository.reserve(&request).await.unwrap().is_none());
            assert_eq!(
                repository.receipt("delivery").await.unwrap().status,
                DeliveryStatus::Pending
            );

            let restarted = ActiveDeliveryService::new(
                repository.clone(),
                gateway.clone(),
                Arc::new(AllowDelivery),
            );
            let receipts = restarted.resume_due(100).await.unwrap();
            assert_eq!(receipts[0].status, DeliveryStatus::Succeeded);
            assert_eq!(*gateway.calls.lock().unwrap(), 1);

            let mut sending = request.clone();
            sending.delivery_id = "unknown".into();
            sending.idempotency_key = "unknown-key".into();
            assert!(repository.reserve(&sending).await.unwrap().is_none());
            let mut lease = repository.receipt("unknown").await.unwrap();
            lease.status = DeliveryStatus::Sending;
            lease.generation = 1;
            lease.lease_expires_at_unix_ms = Some(50);
            repository.save_receipt(lease).await.unwrap();
            assert!(restarted.resume_due(100).await.unwrap().is_empty());
            assert_eq!(
                repository.receipt("unknown").await.unwrap().status,
                DeliveryStatus::ReconcileRequired
            );
            assert_eq!(*gateway.calls.lock().unwrap(), 1);

            let reconciled = restarted.retry("unknown", 110).await.unwrap();
            assert_eq!(reconciled.status, DeliveryStatus::Succeeded);
            assert_eq!(*gateway.calls.lock().unwrap(), 2);

            let mut cancelled = request.clone();
            cancelled.delivery_id = "cancel-reconcile".into();
            cancelled.idempotency_key = "cancel-reconcile-key".into();
            assert!(repository.reserve(&cancelled).await.unwrap().is_none());
            let mut lease = repository.receipt("cancel-reconcile").await.unwrap();
            lease.status = DeliveryStatus::ReconcileRequired;
            lease.generation = 1;
            lease.error_code = Some("delivery.reconcile_required".into());
            repository.save_receipt(lease).await.unwrap();
            assert_eq!(
                restarted.cancel("cancel-reconcile").await.unwrap().status,
                DeliveryStatus::Cancelled
            );
            assert!(restarted.resume_due(200).await.unwrap().is_empty());
        });
    }

    #[test]
    fn duplicate_key_sends_once_and_transient_attempt_resumes_after_restart() {
        futures::executor::block_on(async {
            let repository = Arc::new(Repository::default());
            let gateway = Arc::new(Gateway {
                results: Mutex::new(VecDeque::from([
                    Err(DeliveryFailure {
                        code: "qq.rate_limited".into(),
                        transient: true,
                        retry_after_ms: Some(50),
                        sent_message_ids: Vec::new(),
                        part_receipts: Vec::new(),
                    }),
                    Ok(DeliverySuccess {
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
            let retry = service.submit(&request, 100).await.unwrap();
            assert_eq!(retry.status, DeliveryStatus::RetryScheduled);
            assert!(service.resume_due(149).await.unwrap().is_empty());

            let restarted =
                ActiveDeliveryService::new(repository, gateway.clone(), Arc::new(AllowDelivery));
            let receipts = restarted.resume_due(150).await.unwrap();
            assert_eq!(receipts[0].status, DeliveryStatus::Succeeded);
            let duplicate = restarted.submit(&request, 200).await.unwrap();
            assert_eq!(duplicate.status, DeliveryStatus::Succeeded);
            assert_eq!(*gateway.calls.lock().unwrap(), 2);
        });
    }

    #[test]
    fn in_flight_duplicate_returns_the_original_delivery_identity() {
        let repository = Repository::default();
        let original = request();
        assert!(
            futures::executor::block_on(repository.reserve(&original))
                .unwrap()
                .is_none()
        );
        let mut duplicate = original.clone();
        duplicate.delivery_id = "duplicate-delivery".into();

        let receipt = futures::executor::block_on(repository.reserve(&duplicate))
            .unwrap()
            .unwrap();

        assert_eq!(receipt.delivery_id, original.delivery_id);
        assert_eq!(receipt.status, DeliveryStatus::Pending);
    }

    #[test]
    fn management_operations_expose_history_cancel_due_work_and_retry_failure() {
        futures::executor::block_on(async {
            let repository = Arc::new(Repository::default());
            let gateway = Arc::new(Gateway {
                results: Mutex::new(VecDeque::from([
                    Err(DeliveryFailure {
                        code: "qq.rate_limited".into(),
                        transient: true,
                        retry_after_ms: Some(50),
                        sent_message_ids: Vec::new(),
                        part_receipts: Vec::new(),
                    }),
                    Err(DeliveryFailure {
                        code: "qq.rejected".into(),
                        transient: false,
                        retry_after_ms: None,
                        sent_message_ids: Vec::new(),
                        part_receipts: Vec::new(),
                    }),
                    Ok(DeliverySuccess {
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
                service.submit(&request(), 100).await.unwrap().status,
                DeliveryStatus::RetryScheduled
            );
            let (receipt, attempts) = service.inspect("delivery").await.unwrap();
            assert_eq!(receipt.attempt_count, 1);
            assert_eq!(attempts.len(), 2);
            assert_eq!(
                service.cancel("delivery").await.unwrap().status,
                DeliveryStatus::Cancelled
            );
            assert!(service.resume_due(150).await.unwrap().is_empty());
            assert_eq!(
                service.retry("delivery", 151).await,
                Err(DeliveryError::InvalidState)
            );

            let mut second = request();
            second.delivery_id = "failed-delivery".into();
            second.idempotency_key = "failed-key".into();
            assert_eq!(
                service.submit(&second, 200).await.unwrap().status,
                DeliveryStatus::PermanentlyFailed
            );
            let retried = service.retry("failed-delivery", 201).await.unwrap();
            assert_eq!(retried.status, DeliveryStatus::Succeeded);
            assert_eq!(retried.attempt_count, 2);
            assert_eq!(*gateway.calls.lock().unwrap(), 3);
        });
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
        futures::executor::block_on(async {
            let repository = Arc::new(Repository::default());
            let gateway = Arc::new(Gateway {
                results: Mutex::new(VecDeque::from([Ok(DeliverySuccess {
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

            let first = bridge.deliver(result.clone(), 100).await.unwrap();
            let duplicate = bridge.deliver(result, 101).await.unwrap();

            assert_eq!(first.delivery_id, "agent-scheduled:execution-1");
            assert_eq!(duplicate, first);
            assert_eq!(*gateway.calls.lock().unwrap(), 1);
        });
    }

    #[test]
    fn scheduled_result_rejects_raw_platform_target_kind() {
        futures::executor::block_on(async {
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
                bridge.deliver(result, 100).await,
                Err(DeliveryError::InvalidScheduleTarget)
            );
        });
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
                reply_to: None,
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
