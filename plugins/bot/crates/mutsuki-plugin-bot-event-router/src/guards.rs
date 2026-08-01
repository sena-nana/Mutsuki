use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mutsuki_bot_protocol::{
    BOT_PERMISSION_CHECK_PROTOCOL_ID, BOT_RATE_LIMIT_CHECK_PROTOCOL_ID, BotCooldownScope,
    BotPermission, BotPermissionCheckRequest, BotPermissionCheckResult, BotRateLimitCheckRequest,
    BotRateLimitCheckResult, BotTarget,
};
use mutsuki_runtime_contracts::{
    CompletionBatch, ExecutionClass, PluginManifest, RunnerResult, RuntimeError, Task, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeResult};
use mutsuki_runtime_sdk::{
    PluginBuilder, ProtocolDescriptorBuilder, RunnerDescriptorBuilder, map_work_batch_entries,
};

pub const BOT_HANDLER_GUARD_PLUGIN_ID: &str = "mutsuki.bot.handler.guard";
pub const BOT_PERMISSION_RUNNER_ID: &str = "mutsuki.bot.handler.permission";
pub const BOT_RATE_LIMIT_RUNNER_ID: &str = "mutsuki.bot.handler.rate-limit";

pub trait BotPermissionAuthorizer: Send + Sync {
    fn check(
        &self,
        request: &BotPermissionCheckRequest,
    ) -> Result<BotPermissionCheckResult, String>;
}

#[derive(Clone, Default)]
pub struct StaticBotPermissionAuthorizer {
    actor_grants: BTreeMap<String, BTreeSet<String>>,
    administrator_bypass: bool,
}

impl StaticBotPermissionAuthorizer {
    pub fn new(
        actor_grants: BTreeMap<String, Vec<BotPermission>>,
        administrator_bypass: bool,
    ) -> Self {
        Self {
            actor_grants: actor_grants
                .into_iter()
                .map(|(actor, grants)| {
                    (
                        actor,
                        grants
                            .into_iter()
                            .map(|grant| permission_key(&grant))
                            .collect(),
                    )
                })
                .collect(),
            administrator_bypass,
        }
    }
}

impl BotPermissionAuthorizer for StaticBotPermissionAuthorizer {
    fn check(
        &self,
        request: &BotPermissionCheckRequest,
    ) -> Result<BotPermissionCheckResult, String> {
        if self.administrator_bypass && administrator(&request.event) {
            return Ok(BotPermissionCheckResult {
                allowed: true,
                denial_code: None,
                audit_reason: Some("explicit administrator bypass".into()),
            });
        }
        let Some(actor_id) = request
            .event
            .actor
            .as_ref()
            .map(|actor| actor.user_id.as_str())
        else {
            return Ok(permission_denied("bot.permission.actor_missing"));
        };
        let Some(grants) = self.actor_grants.get(actor_id) else {
            return Ok(permission_denied("bot.permission.actor_unconfigured"));
        };
        let missing = request
            .handler
            .permissions
            .iter()
            .filter(|permission| !grants.contains(&permission_key(permission)))
            .map(permission_key)
            .collect::<Vec<_>>();
        if missing.is_empty() {
            Ok(BotPermissionCheckResult {
                allowed: true,
                denial_code: None,
                audit_reason: Some(format!(
                    "actor {actor_id} satisfied {} permission requirements",
                    request.handler.permissions.len()
                )),
            })
        } else {
            Ok(BotPermissionCheckResult {
                allowed: false,
                denial_code: Some("bot.permission.missing".into()),
                audit_reason: Some(format!("actor {actor_id} lacks {}", missing.join(","))),
            })
        }
    }
}

pub trait BotRateLimitClock: Send + Sync {
    fn now_unix_ms(&self) -> u64;
}

#[derive(Clone, Copy, Default)]
pub struct SystemBotRateLimitClock;

impl BotRateLimitClock for SystemBotRateLimitClock {
    fn now_unix_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or_default()
    }
}

#[derive(Clone)]
pub struct BotRateLimitService {
    clock: Arc<dyn BotRateLimitClock>,
    buckets: Arc<Mutex<BTreeMap<String, TokenBucket>>>,
}

#[derive(Clone, Copy)]
struct TokenBucket {
    available_milli: u128,
    updated_at_ms: u64,
}

impl BotRateLimitService {
    pub fn new(clock: Arc<dyn BotRateLimitClock>) -> Self {
        Self {
            clock,
            buckets: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub fn system() -> Self {
        Self::new(Arc::new(SystemBotRateLimitClock))
    }

    pub fn check(&self, request: &BotRateLimitCheckRequest) -> BotRateLimitCheckResult {
        let policy = &request.policy;
        if policy.capacity == 0 || policy.refill_tokens == 0 || policy.refill_interval_ms == 0 {
            return rate_denied(
                "bot.rate_limit.invalid_policy",
                None,
                "rate-limit capacity, refill and interval must be positive",
            );
        }
        if policy.administrator_bypass && administrator(&request.event) {
            return rate_allowed("explicit administrator bypass");
        }
        let Some(key) = rate_limit_key(request) else {
            return rate_denied(
                "bot.rate_limit.subject_missing",
                None,
                "rate-limit scope requires an actor or supported conversation",
            );
        };
        let now_ms = self.clock.now_unix_ms();
        let capacity = u128::from(policy.capacity) * 1_000;
        let mut buckets = self.buckets.lock().expect("rate-limit mutex poisoned");
        let bucket = buckets.entry(key.clone()).or_insert(TokenBucket {
            available_milli: capacity,
            updated_at_ms: now_ms,
        });
        let elapsed = now_ms.saturating_sub(bucket.updated_at_ms);
        let refill = u128::from(elapsed)
            .saturating_mul(u128::from(policy.refill_tokens))
            .saturating_mul(1_000)
            / u128::from(policy.refill_interval_ms);
        bucket.available_milli = bucket.available_milli.saturating_add(refill).min(capacity);
        bucket.updated_at_ms = now_ms;
        if bucket.available_milli >= 1_000 {
            bucket.available_milli -= 1_000;
            return rate_allowed(&format!("token consumed from {key}"));
        }
        let missing = 1_000_u128.saturating_sub(bucket.available_milli);
        let numerator = missing.saturating_mul(u128::from(policy.refill_interval_ms));
        let denominator = u128::from(policy.refill_tokens).saturating_mul(1_000);
        let retry_after_ms = numerator.div_ceil(denominator).min(u128::from(u64::MAX)) as u64;
        rate_denied(
            "bot.rate_limit.exhausted",
            Some(retry_after_ms.max(1)),
            &format!("token bucket {key} is exhausted"),
        )
    }
}

#[must_use]
pub fn bot_handler_guard_manifest() -> PluginManifest {
    PluginBuilder::new(BOT_HANDLER_GUARD_PLUGIN_ID)
        .runner_descriptor(permission_descriptor())
        .runner_descriptor(rate_limit_descriptor())
        .protocol_handler(
            ProtocolDescriptorBuilder::new(BOT_PERMISSION_CHECK_PROTOCOL_ID).build(),
            BOT_PERMISSION_RUNNER_ID,
            "orchestration",
        )
        .protocol_handler(
            ProtocolDescriptorBuilder::new(BOT_RATE_LIMIT_CHECK_PROTOCOL_ID).build(),
            BOT_RATE_LIMIT_RUNNER_ID,
            "orchestration",
        )
        .build()
        .manifest
}

pub fn permission_runner(authorizer: Arc<dyn BotPermissionAuthorizer>) -> Box<dyn Runner> {
    Box::new(PermissionRunner {
        descriptor: permission_descriptor(),
        authorizer,
    })
}

pub fn rate_limit_runner(service: BotRateLimitService) -> Box<dyn Runner> {
    Box::new(RateLimitRunner {
        descriptor: rate_limit_descriptor(),
        service,
    })
}

struct PermissionRunner {
    descriptor: mutsuki_runtime_contracts::RunnerDescriptor,
    authorizer: Arc<dyn BotPermissionAuthorizer>,
}

impl Runner for PermissionRunner {
    fn descriptor(&self) -> &mutsuki_runtime_contracts::RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, |task| {
            let request: BotPermissionCheckRequest = decode(task)?;
            let output = self
                .authorizer
                .check(&request)
                .map_err(|error| runtime_error(task, error))?;
            completed(task, output)
        })
    }
}

struct RateLimitRunner {
    descriptor: mutsuki_runtime_contracts::RunnerDescriptor,
    service: BotRateLimitService,
}

impl Runner for RateLimitRunner {
    fn descriptor(&self) -> &mutsuki_runtime_contracts::RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, |task| {
            let request: BotRateLimitCheckRequest = decode(task)?;
            completed(task, self.service.check(&request))
        })
    }
}

fn permission_descriptor() -> mutsuki_runtime_contracts::RunnerDescriptor {
    RunnerDescriptorBuilder::new(BOT_PERMISSION_RUNNER_ID, BOT_HANDLER_GUARD_PLUGIN_ID)
        .accepted_protocol(BOT_PERMISSION_CHECK_PROTOCOL_ID)
        .execution_class(ExecutionClass::Orchestration)
        .build()
}

fn rate_limit_descriptor() -> mutsuki_runtime_contracts::RunnerDescriptor {
    RunnerDescriptorBuilder::new(BOT_RATE_LIMIT_RUNNER_ID, BOT_HANDLER_GUARD_PLUGIN_ID)
        .accepted_protocol(BOT_RATE_LIMIT_CHECK_PROTOCOL_ID)
        .execution_class(ExecutionClass::Orchestration)
        .build()
}

fn decode<T: serde::de::DeserializeOwned>(task: &Task) -> Result<T, RuntimeError> {
    serde_json::from_value(task.payload.to_value()).map_err(|error| runtime_error(task, error))
}

fn completed<T: serde::Serialize>(task: &Task, value: T) -> Result<RunnerResult, RuntimeError> {
    let mut result = RunnerResult::completed(task.task_id.clone());
    result.output = Some(serde_json::to_value(value).map_err(|error| runtime_error(task, error))?);
    Ok(result)
}

fn permission_key(permission: &BotPermission) -> String {
    format!("{}\u{0}{}", permission.scope, permission.action)
}

fn permission_denied(code: &str) -> BotPermissionCheckResult {
    BotPermissionCheckResult {
        allowed: false,
        denial_code: Some(code.into()),
        audit_reason: Some(code.into()),
    }
}

fn administrator(event: &mutsuki_bot_protocol::BotEvent) -> bool {
    event
        .ext
        .get("qqbot.actor_role")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|role| matches!(role, "administrator" | "owner"))
}

fn rate_limit_key(request: &BotRateLimitCheckRequest) -> Option<String> {
    let actor = request
        .event
        .actor
        .as_ref()
        .map(|actor| actor.user_id.as_str());
    let conversation = conversation_key(&request.event.target);
    let subject = match request.policy.scope {
        BotCooldownScope::Actor => format!("actor:{}", framed(actor?)),
        BotCooldownScope::Conversation => format!("conversation:{}", conversation?),
        BotCooldownScope::Command => "command".into(),
        BotCooldownScope::ActorAndConversation => {
            format!("actor:{}|conversation:{}", framed(actor?), conversation?)
        }
    };
    Some(format!("handler:{}|{subject}", framed(&request.handler_id)))
}

fn conversation_key(target: &BotTarget) -> Option<String> {
    match target {
        BotTarget::User { user_id } => Some(format!("private:{}", framed(user_id))),
        BotTarget::Group { group_id } => Some(format!("group:{}", framed(group_id))),
        BotTarget::GuildChannel {
            guild_id,
            channel_id,
        } => Some(format!(
            "channel:{}:{}",
            framed(guild_id),
            framed(channel_id)
        )),
        BotTarget::Conversation { conversation_id } => {
            Some(format!("conversation:{}", framed(conversation_id)))
        }
        BotTarget::PlatformSpecific { .. } => None,
    }
}

fn framed(value: &str) -> String {
    format!("{}:{value}", value.len())
}

fn rate_allowed(reason: &str) -> BotRateLimitCheckResult {
    BotRateLimitCheckResult {
        allowed: true,
        retry_after_ms: None,
        denial_code: None,
        audit_reason: Some(reason.into()),
    }
}

fn rate_denied(code: &str, retry_after_ms: Option<u64>, reason: &str) -> BotRateLimitCheckResult {
    BotRateLimitCheckResult {
        allowed: false,
        retry_after_ms,
        denial_code: Some(code.into()),
        audit_reason: Some(reason.into()),
    }
}

fn runtime_error(task: &Task, error: impl std::fmt::Display) -> RuntimeError {
    let mut runtime_error = RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        BOT_HANDLER_GUARD_PLUGIN_ID,
        format!("{}.guard", task.task_id),
    );
    runtime_error.evidence.insert(
        "reason".into(),
        mutsuki_runtime_contracts::ScalarValue::String(error.to_string()),
    );
    runtime_error
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use mutsuki_bot_protocol::{
        BotAccountRef, BotEvent, BotEventKind, BotHandlerDescriptor, BotPlatform,
        BotPropagationPolicy, BotRateLimitPolicy, BotUser,
    };

    use super::*;

    #[derive(Default)]
    struct Clock(AtomicU64);

    impl BotRateLimitClock for Clock {
        fn now_unix_ms(&self) -> u64 {
            self.0.load(Ordering::Relaxed)
        }
    }

    #[test]
    fn token_buckets_are_isolated_by_handler_actor_and_conversation() {
        let clock = Arc::new(Clock::default());
        let service = BotRateLimitService::new(clock.clone());
        let policy = BotRateLimitPolicy {
            scope: BotCooldownScope::ActorAndConversation,
            capacity: 1,
            refill_tokens: 1,
            refill_interval_ms: 1_000,
            administrator_bypass: false,
        };
        let first = rate_request("command-a", "actor-a", "group-a", policy.clone());
        assert!(service.check(&first).allowed);
        let denied = service.check(&first);
        assert!(!denied.allowed);
        assert_eq!(
            denied.denial_code.as_deref(),
            Some("bot.rate_limit.exhausted")
        );
        assert_eq!(denied.retry_after_ms, Some(1_000));
        assert!(
            service
                .check(&rate_request(
                    "command-a",
                    "actor-b",
                    "group-a",
                    policy.clone()
                ))
                .allowed
        );
        assert!(
            service
                .check(&rate_request(
                    "command-a",
                    "actor-a",
                    "group-b",
                    policy.clone()
                ))
                .allowed
        );
        assert!(
            service
                .check(&rate_request(
                    "command-b",
                    "actor-a",
                    "group-a",
                    policy.clone()
                ))
                .allowed
        );
        clock.0.store(1_000, Ordering::Relaxed);
        assert!(service.check(&first).allowed);
    }

    #[test]
    fn permission_requires_all_grants_and_admin_bypass_is_explicit() {
        let permission = BotPermission {
            scope: "bot.config".into(),
            action: "write".into(),
        };
        let authorizer = StaticBotPermissionAuthorizer::new(
            BTreeMap::from([("allowed".into(), vec![permission.clone()])]),
            false,
        );
        let mut handler = handler("config");
        handler.permissions.push(permission);
        assert!(
            authorizer
                .check(&BotPermissionCheckRequest {
                    handler: handler.clone(),
                    event: event("allowed", "group"),
                })
                .unwrap()
                .allowed
        );
        let denied = authorizer
            .check(&BotPermissionCheckRequest {
                handler: handler.clone(),
                event: event("administrator", "group"),
            })
            .unwrap();
        assert!(!denied.allowed);
        let mut admin_event = event("administrator", "group");
        admin_event.ext.insert(
            "qqbot.actor_role".into(),
            serde_json::Value::String("administrator".into()),
        );
        assert!(
            StaticBotPermissionAuthorizer::new(BTreeMap::new(), true)
                .check(&BotPermissionCheckRequest {
                    handler,
                    event: admin_event,
                })
                .unwrap()
                .allowed
        );
    }

    fn rate_request(
        handler_id: &str,
        actor_id: &str,
        group_id: &str,
        policy: BotRateLimitPolicy,
    ) -> BotRateLimitCheckRequest {
        BotRateLimitCheckRequest {
            handler_id: handler_id.into(),
            policy,
            event: event(actor_id, group_id),
        }
    }

    fn handler(id: &str) -> BotHandlerDescriptor {
        BotHandlerDescriptor {
            handler_id: id.into(),
            binding_id: format!("binding-{id}"),
            generation: 1,
            handler_protocol_id: "mutsuki.bot.test/handle@1".into(),
            runner_hint: None,
            event_kinds: vec![BotEventKind::MessageCreated],
            conversation_kinds: Vec::new(),
            filter: None,
            permissions: Vec::new(),
            priority: 0,
            propagation: BotPropagationPolicy::Continue,
            rate_limit: None,
            timeout_ms: None,
            side_effects: Vec::new(),
            max_concurrency: None,
            before_hook_protocol_ids: Vec::new(),
            after_hook_protocol_ids: Vec::new(),
            error_hook_protocol_ids: Vec::new(),
        }
    }

    fn event(actor_id: &str, group_id: &str) -> BotEvent {
        BotEvent {
            event_id: format!("event-{actor_id}-{group_id}"),
            platform: BotPlatform::QqBot,
            bot: BotAccountRef {
                account_id: "main".into(),
                platform: BotPlatform::QqBot,
            },
            kind: BotEventKind::MessageCreated,
            time_ms: 1,
            target: BotTarget::Group {
                group_id: group_id.into(),
            },
            actor: Some(BotUser {
                user_id: actor_id.into(),
                display_name: None,
                avatar_url: None,
            }),
            message: None,
            raw: None,
            ext: Default::default(),
        }
    }
}
