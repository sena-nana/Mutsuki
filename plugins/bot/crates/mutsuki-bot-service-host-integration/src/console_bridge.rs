//! Runtime-local bridge from QQ / Bilibili configured-plugin services to the embedded Web Console.

use std::sync::Arc;

use async_trait::async_trait;
use mutsuki_agent_contracts::{AgentConnectionManagementApi, AgentSessionManagementApi};
use mutsuki_agent_service_host_integration::{
    AGENT_CONNECTION_MANAGEMENT_SERVICE_ID, AGENT_CONNECTION_REGISTRY_SERVICE_ID,
    AgentConnectionManager, AgentConnectionRegistry, LOCAL_AGENT_MANAGEMENT_SERVICE_ID,
    LocalAgentManagementService,
};
use mutsuki_bot_flow::BotFlowRegistry;
use mutsuki_bot_management::{BilibiliManagementApi, QqBotManagementService};
use mutsuki_bot_sandbox::{SandboxApi, SandboxService};
use mutsuki_bot_state_db::BotStateDbRepository;
use mutsuki_plugin_bot_agent::{BOT_AGENT_CONFIG_SERVICE_ID, BotAgentConfigHandle};
use mutsuki_plugin_bot_event_router::BOT_FLOW_REGISTRY_SERVICE_ID;
use mutsuki_runtime_sdk::RuntimeBootstrapperService;
use mutsuki_service_runtime::{ServiceRuntime, ServiceRuntimeHandle};

pub const BILIBILI_MANAGEMENT_SERVICE_ID: &str = "mutsuki.bot.bilibili.management";
pub const QQ_MANAGEMENT_SERVICE_ID: &str = "mutsuki.bot.qq.management";
pub const BOT_STATE_DB_SERVICE_ID: &str = "mutsuki.bot.state.db";
pub use mutsuki_bot_sandbox::SANDBOX_SERVICE_ID;

#[must_use]
pub fn bot_state_db_host_service(
    repository: Arc<BotStateDbRepository>,
) -> RuntimeBootstrapperService {
    RuntimeBootstrapperService::new(BOT_STATE_DB_SERVICE_ID, repository, "bot.state")
}

pub struct BotDatabaseConsoleBridge;

impl BotDatabaseConsoleBridge {
    #[must_use]
    pub fn get(runtime: &ServiceRuntime) -> Option<Arc<BotStateDbRepository>> {
        runtime.host_service(BOT_STATE_DB_SERVICE_ID).ok()
    }
}

pub struct BilibiliConsoleBridge;

impl BilibiliConsoleBridge {
    pub fn get(runtime: &ServiceRuntime) -> Option<Arc<dyn BilibiliManagementApi>> {
        runtime
            .host_service::<Arc<dyn BilibiliManagementApi>>(BILIBILI_MANAGEMENT_SERVICE_ID)
            .ok()
            .map(|api| (*api).clone())
    }
}

pub struct BotAgentConfigConsoleBridge;

impl BotAgentConfigConsoleBridge {
    pub fn get(runtime: &ServiceRuntime) -> Option<Arc<BotAgentConfigHandle>> {
        runtime.host_service(BOT_AGENT_CONFIG_SERVICE_ID).ok()
    }
}

pub struct AgentConnectionConsoleBridge;

impl AgentConnectionConsoleBridge {
    pub fn get(
        runtime: &ServiceRuntime,
    ) -> Option<Arc<dyn Fn() -> Result<Arc<dyn AgentConnectionManagementApi>, String> + Send + Sync>>
    {
        let runtime = runtime.handle();
        Some(Arc::new(move || {
            runtime
                .host_service::<AgentConnectionManager>(AGENT_CONNECTION_MANAGEMENT_SERVICE_ID)
                .map(|manager| manager as Arc<dyn AgentConnectionManagementApi>)
                .map_err(|_| "Agent 状态暂时不可用".into())
        }))
    }
}

pub struct AgentConnectionRegistryConsoleBridge;

impl AgentConnectionRegistryConsoleBridge {
    pub fn get(runtime: &ServiceRuntime) -> Option<AgentConnectionRegistry> {
        runtime
            .host_service::<AgentConnectionRegistry>(AGENT_CONNECTION_REGISTRY_SERVICE_ID)
            .ok()
            .map(|registry| (*registry).clone())
    }
}

pub struct LocalAgentConsoleBridge;

impl LocalAgentConsoleBridge {
    pub fn get(
        runtime: &ServiceRuntime,
    ) -> Option<Arc<dyn Fn() -> Result<Arc<dyn AgentSessionManagementApi>, String> + Send + Sync>>
    {
        let runtime = runtime.handle();
        Some(Arc::new(move || {
            runtime
                .host_service::<LocalAgentManagementService>(LOCAL_AGENT_MANAGEMENT_SERVICE_ID)
                .map(|service| service as Arc<dyn AgentSessionManagementApi>)
                .map_err(|_| "Agent 当前未启用或尚未就绪".into())
        }))
    }
}

pub struct BotFlowConsoleBridge;

impl BotFlowConsoleBridge {
    pub fn get(runtime: &ServiceRuntime) -> Option<Arc<BotFlowRegistry>> {
        runtime.host_service(BOT_FLOW_REGISTRY_SERVICE_ID).ok()
    }
}

pub struct QqConsoleBridge;

struct GenerationAwareQqManagement {
    runtime: ServiceRuntimeHandle,
}

#[async_trait]
impl mutsuki_bot_management::QqBotManagementApi for GenerationAwareQqManagement {
    fn subscribe_changes(&self) -> Option<mutsuki_bot_management::QqManagementChangeSubscription> {
        self.service().ok()?.subscribe_changes()
    }

    async fn snapshot(
        &self,
        query: &str,
        include_secret_status: bool,
    ) -> Result<
        mutsuki_bot_management::QqBotManagementSnapshot,
        mutsuki_bot_management::QqManagementError,
    > {
        self.service()?.snapshot(query, include_secret_status).await
    }

    async fn write(
        &self,
        actor_id: &str,
        request: mutsuki_bot_management::QqManagementWriteRequest,
    ) -> Result<
        mutsuki_bot_management::QqManagementWriteResult,
        mutsuki_bot_management::QqManagementError,
    > {
        self.service()?.write(actor_id, request).await
    }
}

impl GenerationAwareQqManagement {
    fn service(
        &self,
    ) -> Result<Arc<QqBotManagementService>, mutsuki_bot_management::QqManagementError> {
        self.runtime
            .host_service(QQ_MANAGEMENT_SERVICE_ID)
            .map_err(|_| mutsuki_bot_management::QqManagementError {
                code: "qq.owner_unavailable".into(),
                message: "QQ Bot 当前未启用或尚未连接".into(),
            })
    }
}

impl QqConsoleBridge {
    pub fn get(
        runtime: &ServiceRuntime,
    ) -> Option<Arc<dyn mutsuki_bot_management::QqBotManagementApi>> {
        Some(Arc::new(GenerationAwareQqManagement {
            runtime: runtime.handle(),
        }))
    }
}

pub struct SandboxConsoleBridge;

struct GenerationAwareSandbox {
    runtime: ServiceRuntimeHandle,
}

#[async_trait]
impl SandboxApi for GenerationAwareSandbox {
    fn subscribe_changes(&self) -> Option<mutsuki_bot_sandbox::SandboxChangeSubscription> {
        self.service().ok()?.subscribe_changes()
    }

    async fn snapshot(
        &self,
        query: &str,
    ) -> Result<mutsuki_bot_sandbox::SandboxSnapshot, mutsuki_bot_sandbox::SandboxError> {
        self.service()?.snapshot(query).await
    }

    async fn write(
        &self,
        actor_id: &str,
        request: mutsuki_bot_sandbox::SandboxWriteRequest,
    ) -> Result<mutsuki_bot_sandbox::SandboxWriteResult, mutsuki_bot_sandbox::SandboxError> {
        self.service()?.write(actor_id, request).await
    }

    async fn messages(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<mutsuki_bot_sandbox::SandboxMessageView>, mutsuki_bot_sandbox::SandboxError>
    {
        self.service()?.messages(conversation_id).await
    }

    fn observe_event(&self, event: mutsuki_bot_protocol::BotEvent) {
        if let Ok(service) = self.service() {
            service.observe_event(event);
        }
    }

    fn observe_outbound(
        &self,
        conversation: &mutsuki_bot_protocol::QqConversationRef,
        segments: &[mutsuki_bot_protocol::MessageSegment],
        reply_to: Option<&str>,
    ) -> Option<mutsuki_bot_sandbox::SandboxMessageView> {
        self.service()
            .ok()?
            .observe_outbound(conversation, segments, reply_to)
    }

    async fn upload_media(
        &self,
        name: &str,
        mime: &str,
        bytes: Vec<u8>,
    ) -> Result<mutsuki_bot_sandbox::SandboxMediaRef, mutsuki_bot_sandbox::SandboxError> {
        self.service()?.upload_media(name, mime, bytes).await
    }

    async fn media_blob(
        &self,
        media_id: &str,
    ) -> Result<mutsuki_bot_sandbox::SandboxMediaBlob, mutsuki_bot_sandbox::SandboxError> {
        self.service()?.media_blob(media_id).await
    }
}

impl GenerationAwareSandbox {
    fn service(&self) -> Result<Arc<SandboxService>, mutsuki_bot_sandbox::SandboxError> {
        self.runtime.host_service(SANDBOX_SERVICE_ID).map_err(|_| {
            mutsuki_bot_sandbox::SandboxError::new(
                "sandbox.owner_unavailable",
                "沙盒服务当前不可用",
            )
        })
    }
}

impl SandboxConsoleBridge {
    pub fn get(runtime: &ServiceRuntime) -> Option<Arc<dyn SandboxApi>> {
        runtime
            .host_service::<SandboxService>(SANDBOX_SERVICE_ID)
            .ok()?;
        Some(Arc::new(GenerationAwareSandbox {
            runtime: runtime.handle(),
        }))
    }
}
