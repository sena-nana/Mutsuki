use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use mutsuki_bot_delivery::{
    ActiveDeliveryService, DeliveryPolicyResolver, QqDeliveryFailure, QqDeliveryGateway,
    QqDeliverySuccess,
};
use mutsuki_bot_interaction::{InteractionConditionMatcher, InteractionError, InteractionService};
use mutsuki_bot_protocol::{
    BOT_EVENT_INGEST_PROTOCOL_ID, BOT_FLOW_BOT_EVENT_TYPE, BOT_FLOW_INGRESS_PROTOCOL_ID,
    BOT_MESSAGE_SEND_PROTOCOL_ID, BotActiveDeliveryRequest, BotDeliveryContent,
    BotDeliveryPartReceipt, BotEvent, BotFlowContext, BotFlowEventEnvelope, BotFlowPayload,
    BotFlowTypeRef, BotMessage, DeliveryPartStatus, DeliveryPolicy, DeliveryStatus, MessageSegment,
    QqConversationRef,
};
use mutsuki_bot_sandbox::{
    SANDBOX_SERVICE_ID, SandboxApi, SandboxError, SandboxRuntime, SandboxService,
};
use mutsuki_bot_state_db::{
    BotManagementAuditRecord, BotManagementOperationReservation, BotStateDbRepository,
};
use mutsuki_runtime_contracts::{
    ContractSurfaceKind, SurfaceRequirement, Task, TaskOutcome, TaskPayload,
};
use mutsuki_runtime_sdk::ResourceRegistryGateway;
use mutsuki_runtime_sdk::RuntimeClientRef;
use mutsuki_service_runtime::ServiceRuntimeBuilder;
use serde_json::json;

use mutsuki_bot_management::{
    QqAccountViewInput, QqBotManagementService, QqDeliveryView, QqManagementAction,
    QqManagementAuditEntry, QqManagementError, QqManagementOperationReservation, QqManagementPage,
    QqManagementProvider, QqManagementStateStore, account_view_from_config,
};
use mutsuki_plugin_bot_adapter_qqbot::{
    QqAuthManager, QqBotClients, QqBotConfig, QqGatewayMapRunner, QqGatewayMediaHandler,
    QqIdSource, QqMediaProvider, QqOpenApiError, QqOpenApiRunner, ReqwestQqHttpClient,
    ResourceGatewayQqMediaProvider, SharedQqCredentials, qqbot_adapter_manifest,
};
use mutsuki_runtime_sdk::{LoadedPlugin, RuntimeBootstrapperService};
use serde_json::Value;

use crate::console_bridge::{
    BOT_STATE_DB_SERVICE_ID, QQ_MANAGEMENT_SERVICE_ID, bot_state_db_host_service,
};
use crate::event_source::{QqGatewayControlHandle, QqGatewayEventSource, QqGatewayHealthHandle};
use crate::sandbox_intercept::{
    SandboxAwareDeliveryGateway, SandboxAwareOpenApiRunner, SandboxInterceptHandle,
};

type MediaFactory = Arc<
    dyn Fn(Arc<dyn ResourceRegistryGateway>) -> Result<Box<dyn QqMediaProvider>, String>
        + Send
        + Sync,
>;
type IdFactory = Arc<dyn Fn() -> Box<dyn QqIdSource> + Send + Sync>;

/// Complete product bundle for the `hosts/service` assembly.
///
/// The bundle owns only adapter configuration and shared account state. The
/// client secret is populated by `QqGatewayEventSource` through the Host secret
/// boundary when the ServiceRuntime starts.
pub struct QqBotPluginBundle {
    config: QqBotConfig,
    credentials: SharedQqCredentials,
    auth: QqAuthManager,
    health: QqGatewayHealthHandle,
    event_source: Option<QqGatewayEventSource>,
    media_factory: Option<MediaFactory>,
    media_provider_id: Option<String>,
    id_factory: IdFactory,
}

impl QqBotPluginBundle {
    /// Builds the text/recall/account/Gateway bundle without declaring media upload.
    pub fn new(config: QqBotConfig) -> Result<Self, QqOpenApiError> {
        config
            .validate()
            .map_err(|error| QqOpenApiError::InvalidPayload(error.to_string()))?;
        let credentials = SharedQqCredentials::default();
        let auth = QqAuthManager::new();
        let event_source =
            QqGatewayEventSource::new(config.clone(), credentials.clone(), auth.clone());
        let health = event_source.health_handle();
        Ok(Self {
            config,
            credentials,
            auth,
            health,
            event_source: Some(event_source),
            media_factory: None,
            media_provider_id: None,
            id_factory: Arc::new(|| Box::new(SystemQqIdSource::new())),
        })
    }

    /// Enables media upload only when a real resource provider is available.
    pub fn with_media_provider<F>(mut self, media_factory: F) -> Self
    where
        F: Fn() -> Box<dyn QqMediaProvider> + Send + Sync + 'static,
    {
        self.media_factory = Some(Arc::new(move |_resources| Ok(media_factory())));
        self
    }

    pub fn with_resource_media_provider(mut self, provider_id: impl Into<String>) -> Self {
        let provider_id = provider_id.into();
        self.media_provider_id = Some(provider_id.clone());
        self.media_factory = Some(Arc::new(move |resources| {
            ResourceGatewayQqMediaProvider::new(provider_id.clone(), resources)
                .map(|provider| Box::new(provider) as Box<dyn QqMediaProvider>)
                .map_err(|error| error.to_string())
        }));
        self
    }

    pub fn with_id_source_factory<F>(mut self, factory: F) -> Self
    where
        F: Fn() -> Box<dyn QqIdSource> + Send + Sync + 'static,
    {
        self.id_factory = Arc::new(factory);
        self
    }

    pub fn health_handle(&self) -> QqGatewayHealthHandle {
        self.health.clone()
    }

    pub fn install(
        mut self,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, QqOpenApiError> {
        let gateway_config = self.config.clone();
        let openapi_config = self.config.clone();
        let credentials = self.credentials.clone();
        let auth = self.auth.clone();
        let media_factory = self.media_factory.clone();
        let media_provider_id = self.media_provider_id.clone();
        let media_enabled = media_factory.is_some();
        let id_factory = self.id_factory.clone();
        let health = self.health.clone();
        let health_component_id = format!("mutsuki.bot.qqbot.gateway:{}", self.config.account_id);
        let source = self
            .event_source
            .take()
            .ok_or_else(|| QqOpenApiError::InvalidPayload("event source already taken".into()))?;
        let control = source.control_handle();
        let inbound = source.inbound_handle();
        let intercept = SandboxInterceptHandle::default();
        let intercept_for_factory = intercept.clone();
        let intercept_for_runner = intercept.clone();
        let management_config = self.config.clone();
        let management_health = self.health.clone();
        let management_credentials = self.credentials.clone();
        let state_dir = builder.data_dir().join("bot");
        std::fs::create_dir_all(&state_dir).map_err(|error| {
            QqOpenApiError::InvalidPayload(format!(
                "failed to create QQ management state directory: {error}"
            ))
        })?;
        let state_path = state_dir.join("state.sqlite3");
        let mut manifest = qqbot_adapter_manifest(1, media_enabled);
        manifest
            .provides
            .services
            .push(QQ_MANAGEMENT_SERVICE_ID.into());
        manifest.provides.services.push(SANDBOX_SERVICE_ID.into());
        manifest
            .provides
            .services
            .push(BOT_STATE_DB_SERVICE_ID.into());
        manifest
            .provides
            .capabilities
            .push("bot.qq.management".into());
        manifest.provides.capabilities.push("bot.sandbox".into());
        manifest.provides.capabilities.push("bot.state".into());
        if let Some(provider_id) = media_provider_id {
            manifest.requires.push(SurfaceRequirement::new(
                ContractSurfaceKind::ResourceProvider,
                provider_id,
            ));
        }
        let loaded_manifest = manifest.clone();
        let builder =
            builder.register_runtime_client_loaded_plugin_factory(manifest, move |runtime| {
                let repository = Arc::new(
                    BotStateDbRepository::open(&state_path).map_err(|error| error.to_string())?,
                );
                let gateway = Arc::new(SandboxAwareDeliveryGateway::new(
                    Arc::new(RuntimeQqDeliveryGateway {
                        runtime: runtime.clone(),
                        account_id: management_config.account_id.clone(),
                    }),
                    intercept_for_factory.clone(),
                ));
                let delivery = ActiveDeliveryService::new(
                    repository.clone(),
                    gateway,
                    Arc::new(ConfiguredAccountDeliveryPolicy {
                        account_id: management_config.account_id.clone(),
                    }),
                );
                let interaction = InteractionService::new(
                    repository.clone(),
                    Arc::new(ManagementInteractionMatcher),
                );
                let provider = Arc::new(OwnerBackedQqManagementProvider {
                    config: management_config.clone(),
                    credentials: management_credentials.clone(),
                    health: management_health.clone(),
                    control: control.clone(),
                    repository: repository.clone(),
                    delivery: delivery.clone(),
                    interaction,
                });
                let state = Arc::new(StateDbQqManagementStore {
                    repository: repository.clone(),
                });
                let management =
                    Arc::new(QqBotManagementService::with_state_store(provider, state));
                let sandbox = Arc::new(
                    SandboxService::with_history(
                        management_config.account_id.clone(),
                        repository.clone(),
                    )
                    .map_err(|error| error.to_string())?,
                );
                sandbox.set_runtime(Arc::new(HostSandboxRuntime {
                    runtime,
                    delivery,
                    account_id: management_config.account_id.clone(),
                }));
                intercept_for_factory.bind(sandbox.clone());
                let observed = sandbox.clone();
                inbound.set(Arc::new(move |event| observed.observe_event(event)));
                let mut host_services = vec![
                    RuntimeBootstrapperService::new(
                        QQ_MANAGEMENT_SERVICE_ID,
                        management,
                        "bot.qq.management",
                    ),
                    RuntimeBootstrapperService::new(SANDBOX_SERVICE_ID, sandbox, "bot.sandbox"),
                ];
                host_services.push(bot_state_db_host_service(repository));
                Ok::<LoadedPlugin, String>(LoadedPlugin {
                    manifest: loaded_manifest.clone(),
                    runners: Vec::new(),
                    async_handlers: Vec::new(),
                    host_services,
                    resource_providers: Vec::new(),
                    async_resource_providers: Vec::new(),
                    host_effects: Vec::new(),
                })
            });
        let builder = if media_enabled {
            builder.register_fallible_runtime_services_async_handler(move |_runtime, resources| {
                QqGatewayMediaHandler::new(gateway_config.clone(), resources).map(|handler| {
                    Arc::new(handler) as Arc<dyn mutsuki_runtime_core::AsyncBatchHandler>
                })
            })
        } else {
            builder.register_builtin_runner(move || {
                Box::new(QqGatewayMapRunner::with_app_id(
                    1,
                    gateway_config.account_id.clone(),
                    gateway_config.app_id.clone(),
                ))
            })
        };
        Ok(builder
            .register_fallible_runtime_services_runner(move |_runtime, resources| {
                let http = ReqwestQqHttpClient::new(&openapi_config)
                    .map_err(|error| error.redacted_message())?;
                Ok::<Box<dyn mutsuki_runtime_core::Runner>, String>(Box::new(
                    SandboxAwareOpenApiRunner::new(
                        Box::new(QqOpenApiRunner::new_with_auth(
                            1,
                            openapi_config.clone(),
                            {
                                let clients = QqBotClients::new(
                                    Box::new(http),
                                    Arc::new(credentials.clone()),
                                );
                                match &media_factory {
                                    Some(factory) => {
                                        clients.with_media_provider(factory(resources)?)
                                    }
                                    None => clients,
                                }
                            },
                            id_factory(),
                            auth.clone(),
                        )),
                        intercept_for_runner.clone(),
                        openapi_config.account_id.clone(),
                    ),
                ))
            })
            .register_health_probe(health_component_id, move || {
                let snapshot = health.snapshot();
                json!({
                    "status": if snapshot.connected && snapshot.identified {
                        "ok"
                    } else if snapshot.connected {
                        "degraded"
                    } else {
                        "unhealthy"
                    },
                    "connected": snapshot.connected,
                    "identified": snapshot.identified,
                    "last_heartbeat_unix_ms": snapshot.last_heartbeat_unix_ms,
                    "last_ack_unix_ms": snapshot.last_ack_unix_ms,
                    "last_event_unix_ms": snapshot.last_event_unix_ms,
                    "reconnect_count": snapshot.reconnect_count,
                    "last_error": snapshot.last_error,
                    "last_error_code": snapshot.last_error_code,
                    "started_at_unix_ms": snapshot.started_at_unix_ms,
                    "connected_since_unix_ms": snapshot.connected_since_unix_ms,
                })
            })
            .register_event_source(Box::new(source)))
    }
}

struct OwnerBackedQqManagementProvider {
    config: QqBotConfig,
    credentials: SharedQqCredentials,
    health: QqGatewayHealthHandle,
    control: QqGatewayControlHandle,
    repository: Arc<BotStateDbRepository>,
    delivery: ActiveDeliveryService,
    interaction: InteractionService,
}

impl OwnerBackedQqManagementProvider {
    fn account_view(&self, include_secret_status: bool) -> mutsuki_bot_management::QqAccountView {
        let snapshot = self.health.snapshot();
        let mut account = account_view_from_config(QqAccountViewInput {
            account_id: self.config.account_id.clone(),
            app_id: self.config.app_id.clone(),
            credential_reference: self.config.client_secret_key.clone(),
            credential_present: self.credentials.is_configured(),
            capability: self.config.capability_matrix(),
            intents: self.config.gateway_intents,
            shard: self.config.shard,
            connected: snapshot.connected,
            identified: snapshot.identified,
            last_heartbeat_unix_ms: snapshot
                .last_heartbeat_unix_ms
                .map(|value| u64::try_from(value).unwrap_or(u64::MAX)),
            last_error: snapshot.last_error,
            last_error_code: snapshot.last_error_code,
            reconnect_count: snapshot.reconnect_count,
            self_user: snapshot.self_user,
        });
        if !include_secret_status {
            account.credential_reference.clear();
            account.credential_status = "restricted".into();
        }
        account
    }

    fn require_account(&self, account_id: &str) -> Result<(), QqManagementError> {
        (account_id == self.config.account_id)
            .then_some(())
            .ok_or_else(|| QqManagementError {
                code: "not_found".into(),
                message: format!("QQ 账号 `{account_id}` 不存在"),
            })
    }

    fn find_interaction(
        &self,
        session_id: &str,
    ) -> Result<mutsuki_bot_protocol::BotInteractionSession, QqManagementError> {
        let mut cursor = None;
        loop {
            let page = self
                .repository
                .interaction_page(cursor.as_deref(), 100)
                .map_err(state_error)?;
            if let Some(session) = page
                .items
                .into_iter()
                .find(|session| session.session_id == session_id)
            {
                return Ok(session);
            }
            let Some(next) = page.next_cursor else {
                return Err(QqManagementError {
                    code: "not_found".into(),
                    message: format!("interaction `{session_id}` 不存在"),
                });
            };
            cursor = Some(next);
        }
    }
}

#[async_trait]
impl QqManagementProvider for OwnerBackedQqManagementProvider {
    fn load_snapshot(
        &self,
        query: &str,
        include_secret_status: bool,
    ) -> Result<mutsuki_bot_management::QqBotManagementSnapshot, QqManagementError> {
        let query_lower = query.trim().to_ascii_lowercase();
        let account = self.account_view(include_secret_status);
        let accounts = (query_lower.is_empty()
            || account
                .account_id
                .to_ascii_lowercase()
                .contains(&query_lower))
        .then_some(account)
        .into_iter()
        .collect();
        Ok(mutsuki_bot_management::QqBotManagementSnapshot {
            revision: 0,
            accounts,
            deliveries: self.delivery_page(query, None, 50)?.items,
            interactions: self.interaction_page(query, None, 50)?.items,
        })
    }

    async fn apply(
        &self,
        operation_id: &str,
        replaying: bool,
        _actor_id: &str,
        action: &QqManagementAction,
    ) -> Result<Value, QqManagementError> {
        match action {
            QqManagementAction::AccountSetEnabled { .. } => Err(QqManagementError {
                code: "configuration.required".into(),
                message: "请在配置里启用或停用账号".into(),
            }),
            QqManagementAction::AccountHealthCheck { account_id } => {
                self.require_account(account_id)?;
                serde_json::to_value(self.account_view(false)).map_err(encode_error)
            }
            QqManagementAction::AccountReconnect { account_id } => {
                self.require_account(account_id)?;
                self.control
                    .reconnect()
                    .map_err(|message| QqManagementError {
                        code: "qq.gateway_unavailable".into(),
                        message,
                    })?;
                Ok(json!({ "account_id": account_id, "reconnect_requested": true }))
            }
            QqManagementAction::AccountSendTest {
                account_id,
                conversation,
                text,
            } => {
                self.require_account(account_id)?;
                if conversation.account_id != *account_id {
                    return Err(QqManagementError {
                        code: "invalid_argument".into(),
                        message: "测试会话不属于当前 QQ 账号".into(),
                    });
                }
                if text.trim().is_empty() {
                    return Err(QqManagementError {
                        code: "invalid_argument".into(),
                        message: "测试消息不能为空".into(),
                    });
                }
                let request = BotActiveDeliveryRequest {
                    delivery_id: format!("qq-test-{operation_id}"),
                    idempotency_key: format!("qq-test-key-{operation_id}"),
                    conversation: conversation.clone(),
                    content: BotDeliveryContent {
                        segments: vec![MessageSegment::text(text.clone())],
                        summary: Some("QQ 测试消息".into()),
                        reply_to: None,
                    },
                    policy: DeliveryPolicy {
                        max_attempts: 1,
                        initial_backoff_ms: 1_000,
                        max_backoff_ms: 1_000,
                        not_before_unix_ms: None,
                        expires_at_unix_ms: None,
                    },
                    dry_run: false,
                    source_execution_id: None,
                };
                let now = unix_ms();
                let receipt = self
                    .delivery
                    .submit(&request, now)
                    .await
                    .map_err(delivery_error)?;
                serde_json::to_value(receipt).map_err(encode_error)
            }
            QqManagementAction::DeliveryRetry { delivery_id } => {
                let receipt = match self.delivery.retry(delivery_id, unix_ms()).await {
                    Ok(receipt) => receipt,
                    Err(mutsuki_bot_delivery::DeliveryError::InvalidState) if replaying => self
                        .delivery
                        .inspect(delivery_id)
                        .await
                        .map(|(receipt, _)| receipt)
                        .map_err(delivery_error)?,
                    Err(error) => return Err(delivery_error(error)),
                };
                serde_json::to_value(receipt).map_err(encode_error)
            }
            QqManagementAction::DeliveryCancel { delivery_id } => {
                let receipt = match self.delivery.cancel(delivery_id).await {
                    Ok(receipt) => receipt,
                    Err(mutsuki_bot_delivery::DeliveryError::InvalidState) if replaying => self
                        .delivery
                        .inspect(delivery_id)
                        .await
                        .map(|(receipt, _)| receipt)
                        .map_err(delivery_error)?,
                    Err(error) => return Err(delivery_error(error)),
                };
                serde_json::to_value(receipt).map_err(encode_error)
            }
            QqManagementAction::DeliveryPreview { delivery_id } => {
                let receipt = self
                    .delivery
                    .preview(delivery_id)
                    .await
                    .map_err(delivery_error)?;
                serde_json::to_value(receipt).map_err(encode_error)
            }
            QqManagementAction::InteractionCancel { session_id } => {
                let session = self.find_interaction(session_id)?;
                let next_version = session.version.saturating_add(1);
                if replaying && session.status == mutsuki_bot_protocol::InteractionStatus::Cancelled
                {
                    return Ok(json!({
                        "session_id": session_id,
                        "status": "cancelled",
                        "version": session.version,
                    }));
                }
                self.interaction
                    .cancel(session)
                    .await
                    .map_err(interaction_error)?;
                Ok(json!({
                    "session_id": session_id,
                    "status": "cancelled",
                    "version": next_version,
                }))
            }
        }
    }

    fn delivery_page(
        &self,
        query: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<QqManagementPage<QqDeliveryView>, QqManagementError> {
        let page = self
            .repository
            .delivery_page(after, limit)
            .map_err(state_error)?;
        let query = query.trim().to_ascii_lowercase();
        Ok(QqManagementPage {
            items: page
                .items
                .into_iter()
                .map(|(receipt, attempts)| QqDeliveryView { receipt, attempts })
                .filter(|item| {
                    query.is_empty()
                        || item
                            .receipt
                            .delivery_id
                            .to_ascii_lowercase()
                            .contains(&query)
                })
                .collect(),
            next_cursor: page.next_cursor,
        })
    }

    fn interaction_page(
        &self,
        query: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<QqManagementPage<mutsuki_bot_protocol::BotInteractionSession>, QqManagementError>
    {
        let page = self
            .repository
            .interaction_page(after, limit)
            .map_err(state_error)?;
        let query = query.trim().to_ascii_lowercase();
        Ok(QqManagementPage {
            items: page
                .items
                .into_iter()
                .filter(|session| {
                    query.is_empty() || session.session_id.to_ascii_lowercase().contains(&query)
                })
                .collect(),
            next_cursor: page.next_cursor,
        })
    }
}

struct StateDbQqManagementStore {
    repository: Arc<BotStateDbRepository>,
}

impl QqManagementStateStore for StateDbQqManagementStore {
    fn revision(&self) -> Result<u64, QqManagementError> {
        self.repository.management_revision().map_err(state_error)
    }

    fn begin_operation(
        &self,
        operation_id: &str,
        expected_revision: u64,
        actor_id: &str,
        action: &str,
        created_at_unix_ms: u64,
    ) -> Result<QqManagementOperationReservation, QqManagementError> {
        self.repository
            .begin_management_operation(
                operation_id,
                expected_revision,
                actor_id,
                action,
                created_at_unix_ms,
            )
            .map(|reservation| match reservation {
                BotManagementOperationReservation::Reserved => {
                    QqManagementOperationReservation::Reserved
                }
                BotManagementOperationReservation::Resume => {
                    QqManagementOperationReservation::Resume
                }
                BotManagementOperationReservation::Completed(record) => {
                    QqManagementOperationReservation::Completed(management_audit(record))
                }
            })
            .map_err(state_error)
    }

    fn complete_operation(
        &self,
        operation_id: &str,
        action: &str,
        result: Value,
        created_at_unix_ms: u64,
    ) -> Result<QqManagementAuditEntry, QqManagementError> {
        self.repository
            .complete_management_operation(operation_id, action, result, created_at_unix_ms)
            .map(management_audit)
            .map_err(state_error)
    }

    fn audits(&self) -> Result<Vec<QqManagementAuditEntry>, QqManagementError> {
        self.repository
            .management_audits(100)
            .map(|records| records.into_iter().map(management_audit).collect())
            .map_err(state_error)
    }
}

fn management_audit(record: BotManagementAuditRecord) -> QqManagementAuditEntry {
    QqManagementAuditEntry {
        audit_id: record.audit_id,
        actor_id: record.actor_id,
        action: record.action,
        revision: record.revision,
        result: record.result,
        created_at_unix_ms: record.created_at_unix_ms,
    }
}

struct RuntimeQqDeliveryGateway {
    runtime: RuntimeClientRef,
    account_id: String,
}

impl QqDeliveryGateway for RuntimeQqDeliveryGateway {
    fn send(
        &self,
        conversation: &QqConversationRef,
        content: &BotDeliveryContent,
    ) -> Result<QqDeliverySuccess, QqDeliveryFailure> {
        if conversation.account_id != self.account_id {
            return Err(qq_delivery_failure(
                "qq.account_mismatch",
                false,
                Vec::new(),
            ));
        }
        let target = conversation
            .target()
            .ok_or_else(|| qq_delivery_failure("qq.conversation.invalid", false, Vec::new()))?;
        let message = BotMessage {
            message_id: None,
            target,
            sender: None,
            segments: content.segments.clone(),
            reply_to: content.reply_to.clone(),
            time_ms: None,
            ext: Default::default(),
        };
        let task = Task::new(
            unique_id("qq-management-send"),
            BOT_MESSAGE_SEND_PROTOCOL_ID,
            serde_json::to_value(message)
                .map_err(|_| qq_delivery_failure("qq.message.encode", false, Vec::new()))?,
        );
        let handle = self.runtime.submit_one(task).map_err(|error| {
            qq_delivery_failure(&format!("qq.runtime.submit:{error}"), true, Vec::new())
        })?;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match self.runtime.task_outcome(&handle) {
                Ok(Some(TaskOutcome::Completed { output, .. })) => {
                    let mut message_ids = Vec::new();
                    if let Some(output) = output {
                        collect_message_ids(&output, &mut message_ids);
                    }
                    if message_ids.is_empty() {
                        return Err(qq_delivery_failure(
                            "qq.response.message_id_missing",
                            false,
                            Vec::new(),
                        ));
                    }
                    let part_receipts = message_ids
                        .iter()
                        .enumerate()
                        .map(|(index, message_id)| BotDeliveryPartReceipt {
                            part_index: index as u32,
                            status: DeliveryPartStatus::Succeeded,
                            platform_message_id: Some(message_id.clone()),
                            error_code: None,
                        })
                        .collect();
                    return Ok(QqDeliverySuccess {
                        platform_message_ids: message_ids,
                        part_receipts,
                    });
                }
                Ok(Some(TaskOutcome::Failed { error, .. })) => {
                    let transient = error.code.contains("rate")
                        || error.code.contains("transient")
                        || error.code.contains("timeout");
                    return Err(qq_delivery_failure(&error.code, transient, Vec::new()));
                }
                Ok(Some(other)) => {
                    return Err(qq_delivery_failure(
                        &format!("qq.runtime.{other:?}"),
                        false,
                        Vec::new(),
                    ));
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Ok(None) => {
                    return Err(qq_delivery_failure("qq.runtime.timeout", true, Vec::new()));
                }
                Err(error) => {
                    return Err(qq_delivery_failure(
                        &format!("qq.runtime.outcome:{error}"),
                        true,
                        Vec::new(),
                    ));
                }
            }
        }
    }
}

struct ConfiguredAccountDeliveryPolicy {
    account_id: String,
}

impl DeliveryPolicyResolver for ConfiguredAccountDeliveryPolicy {
    fn active_delivery_allowed(
        &self,
        conversation: &QqConversationRef,
    ) -> Result<bool, mutsuki_bot_delivery::DeliveryError> {
        Ok(conversation.account_id == self.account_id)
    }
}

struct ManagementInteractionMatcher;

impl InteractionConditionMatcher for ManagementInteractionMatcher {
    fn command_matches(
        &self,
        _command: &str,
        _event: &mutsuki_bot_protocol::BotEvent,
    ) -> Result<bool, InteractionError> {
        Ok(false)
    }

    fn predicate_matches(
        &self,
        _service_id: &str,
        _event: &mutsuki_bot_protocol::BotEvent,
    ) -> Result<bool, InteractionError> {
        Ok(false)
    }
}

fn delivery_error(error: mutsuki_bot_delivery::DeliveryError) -> QqManagementError {
    let code = match error {
        mutsuki_bot_delivery::DeliveryError::NotFound => "not_found",
        mutsuki_bot_delivery::DeliveryError::InvalidState => "invalid_state",
        mutsuki_bot_delivery::DeliveryError::PolicyDenied => "policy_denied",
        mutsuki_bot_delivery::DeliveryError::Conflict => "revision.conflict",
        _ => "qq.delivery_failed",
    };
    QqManagementError {
        code: code.into(),
        message: error.to_string(),
    }
}

fn interaction_error(error: InteractionError) -> QqManagementError {
    let code = match error {
        InteractionError::NotWaiting => "invalid_state",
        InteractionError::GenerationConflict => "revision.conflict",
        _ => "qq.interaction_failed",
    };
    QqManagementError {
        code: code.into(),
        message: error.to_string(),
    }
}

fn state_error(error: mutsuki_bot_state_db::BotStateDbError) -> QqManagementError {
    let code = if matches!(&error, mutsuki_bot_state_db::BotStateDbError::Conflict) {
        "operation.conflict"
    } else {
        "qq.state_unavailable"
    };
    QqManagementError {
        code: code.into(),
        message: error.to_string(),
    }
}

fn encode_error(error: serde_json::Error) -> QqManagementError {
    QqManagementError {
        code: "qq.management_encode".into(),
        message: error.to_string(),
    }
}

fn qq_delivery_failure(
    code: &str,
    transient: bool,
    sent_message_ids: Vec<String>,
) -> QqDeliveryFailure {
    QqDeliveryFailure {
        code: code.into(),
        transient,
        retry_after_ms: None,
        sent_message_ids,
        part_receipts: Vec::new(),
    }
}

fn collect_message_ids(value: &Value, message_ids: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            if let Some(id) = object.get("id").and_then(Value::as_str)
                && !id.is_empty()
                && !message_ids.iter().any(|existing| existing == id)
            {
                message_ids.push(id.into());
            }
            for child in object.values() {
                collect_message_ids(child, message_ids);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_message_ids(child, message_ids);
            }
        }
        _ => {}
    }
}

fn unique_id(prefix: &str) -> String {
    format!("{prefix}-{}-{:016x}", unix_ms(), fastrand::u64(..))
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
}

struct HostSandboxRuntime {
    runtime: RuntimeClientRef,
    delivery: ActiveDeliveryService,
    account_id: String,
}

#[async_trait]
impl SandboxRuntime for HostSandboxRuntime {
    fn live_available(&self) -> bool {
        true
    }

    async fn ingest(&self, event: BotEvent) -> Result<(), SandboxError> {
        let event_id = event.event_id.clone();
        let envelope = BotFlowEventEnvelope {
            event_id: event.event_id.clone(),
            protocol_id: BOT_EVENT_INGEST_PROTOCOL_ID.into(),
            payload: BotFlowPayload {
                event_type: BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1),
                value: serde_json::to_value(&event)
                    .map_err(|error| SandboxError::new("encode_failed", error.to_string()))?,
            },
            context: BotFlowContext {
                bot: Some(event.bot.clone()),
                target: Some(event.target.clone()),
                actor: event.actor.clone(),
                ext: event.ext.clone(),
            },
            trace_id: None,
            correlation_id: Some(event_id.clone()),
        };
        let task = Task::new(
            format!("sandbox.ingress:{event_id}"),
            BOT_FLOW_INGRESS_PROTOCOL_ID,
            TaskPayload::from_local(envelope),
        );
        self.runtime
            .submit_one(task)
            .map_err(|error| SandboxError::new("runtime.submit", error.to_string()))?;
        Ok(())
    }

    async fn deliver(
        &self,
        operation_id: &str,
        conversation: &QqConversationRef,
        segments: &[MessageSegment],
        reply_to: Option<&str>,
    ) -> Result<Value, SandboxError> {
        if conversation.account_id != self.account_id {
            return Err(SandboxError::new(
                "invalid_argument",
                "会话不属于当前 QQ 账号",
            ));
        }
        let request = BotActiveDeliveryRequest {
            delivery_id: format!("sandbox-send-{operation_id}"),
            idempotency_key: format!("sandbox-send-key-{operation_id}"),
            conversation: conversation.clone(),
            content: BotDeliveryContent {
                segments: segments.to_vec(),
                summary: Some("沙盒后台消息".into()),
                reply_to: reply_to.map(str::to_owned),
            },
            policy: DeliveryPolicy {
                max_attempts: 1,
                initial_backoff_ms: 1_000,
                max_backoff_ms: 1_000,
                not_before_unix_ms: None,
                expires_at_unix_ms: None,
            },
            dry_run: false,
            source_execution_id: None,
        };
        let receipt = self
            .delivery
            .submit(&request, unix_ms())
            .await
            .map_err(|error| SandboxError::new("delivery.failed", error.to_string()))?;
        if receipt.status != DeliveryStatus::Succeeded {
            let code = receipt
                .error_code
                .clone()
                .unwrap_or_else(|| "delivery.failed".into());
            return Err(SandboxError::new(
                code.clone(),
                format!("真实消息发送失败（{code}）"),
            ));
        }
        serde_json::to_value(receipt)
            .map_err(|error| SandboxError::new("encode_failed", error.to_string()))
    }
}

struct SystemQqIdSource {
    next: u16,
}

impl SystemQqIdSource {
    fn new() -> Self {
        let next = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u16)
            .unwrap_or(1);
        Self { next }
    }

    #[cfg(test)]
    fn from_seed(next: u16) -> Self {
        Self { next }
    }
}

impl QqIdSource for SystemQqIdSource {
    fn next_msg_seq(&mut self) -> u64 {
        let current = u64::from(self.next);
        self.next = self.next.wrapping_add(1);
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use mutsuki_bot_protocol::{BotConversationKind, QQ_CONVERSATION_REF_VERSION};
    use mutsuki_runtime_contracts::{CancelPolicy, TaskBatch, TaskHandle};

    struct CompletedRuntime {
        submitted: Arc<Mutex<Vec<Task>>>,
    }

    impl mutsuki_runtime_sdk::RuntimeClient for CompletedRuntime {
        fn submit_batch(
            &self,
            batch: TaskBatch,
        ) -> mutsuki_runtime_core::RuntimeResult<Vec<TaskHandle>> {
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
            self.submitted.lock().unwrap().extend(batch.tasks);
            Ok(handles)
        }

        fn task_outcome(
            &self,
            handle: &TaskHandle,
        ) -> mutsuki_runtime_core::RuntimeResult<Option<TaskOutcome>> {
            Ok(Some(TaskOutcome::Completed {
                task_id: handle.task_id.clone(),
                output: Some(json!({"id": "QQ_MESSAGE_ID"})),
                output_ref: None,
            }))
        }
    }

    #[test]
    fn system_message_sequence_stays_within_qq_unsigned_16_bit_range() {
        let mut source = SystemQqIdSource::from_seed(u16::MAX);

        assert_eq!(source.next_msg_seq(), u64::from(u16::MAX));
        assert_eq!(source.next_msg_seq(), 0);
        assert_eq!(source.next_msg_seq(), 1);
    }

    #[test]
    fn management_delivery_uses_the_real_qq_typed_task_and_platform_receipt() {
        let submitted = Arc::new(Mutex::new(Vec::new()));
        let gateway = RuntimeQqDeliveryGateway {
            runtime: Arc::new(CompletedRuntime {
                submitted: submitted.clone(),
            }),
            account_id: "main".into(),
        };
        let success = gateway
            .send(
                &QqConversationRef {
                    version: QQ_CONVERSATION_REF_VERSION,
                    account_id: "main".into(),
                    kind: BotConversationKind::Private,
                    user_id: Some("user-openid".into()),
                    group_id: None,
                    guild_id: None,
                    channel_id: None,
                    thread_id: None,
                },
                &BotDeliveryContent {
                    segments: vec![MessageSegment::text("hello")],
                    summary: None,
                    reply_to: None,
                },
            )
            .unwrap();

        assert_eq!(success.platform_message_ids, vec!["QQ_MESSAGE_ID"]);
        let tasks = submitted.lock().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].protocol_id, BOT_MESSAGE_SEND_PROTOCOL_ID);
        let message: BotMessage = serde_json::from_value(tasks[0].payload.to_value()).unwrap();
        assert_eq!(message.plain_text(), "hello");
    }
}
