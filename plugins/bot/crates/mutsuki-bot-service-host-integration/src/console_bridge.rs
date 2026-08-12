//! Runtime-local bridge from QQ / Bilibili configured-plugin services to the embedded Web Console.

use std::sync::Arc;

use mutsuki_agent_service_host_integration::{
    AGENT_CONNECTION_MANAGEMENT_SERVICE_ID, AGENT_CONNECTION_REGISTRY_SERVICE_ID,
    AgentConnectionManager, AgentConnectionRegistry,
};
use mutsuki_bot_flow::BotFlowRegistry;
use mutsuki_plugin_bot_agent::{BOT_AGENT_CONFIG_SERVICE_ID, BotAgentConfigHandle};
use mutsuki_plugin_bot_bilibili::BilibiliManagementService;
use mutsuki_plugin_bot_event_router::BOT_FLOW_REGISTRY_SERVICE_ID;
use mutsuki_plugin_bot_qq_web::QqBotManagementService;
use mutsuki_service_runtime::{ServiceRuntime, ServiceRuntimeHandle};

pub const BILIBILI_MANAGEMENT_SERVICE_ID: &str = "mutsuki.bot.bilibili.management";
pub const QQ_MANAGEMENT_SERVICE_ID: &str = "mutsuki.bot.qq.management";

pub struct BilibiliConsoleBridge;

impl BilibiliConsoleBridge {
    pub fn get(runtime: &ServiceRuntime) -> Option<Arc<BilibiliManagementService>> {
        runtime.host_service(BILIBILI_MANAGEMENT_SERVICE_ID).ok()
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
    ) -> Option<Arc<dyn Fn() -> Result<Arc<AgentConnectionManager>, String> + Send + Sync>> {
        let runtime = runtime.handle();
        Some(Arc::new(move || {
            runtime
                .host_service(AGENT_CONNECTION_MANAGEMENT_SERVICE_ID)
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

impl mutsuki_plugin_bot_qq_web::QqBotManagementApi for GenerationAwareQqManagement {
    fn snapshot(
        &self,
        query: &str,
        include_secret_status: bool,
    ) -> Result<
        mutsuki_plugin_bot_qq_web::QqBotManagementSnapshot,
        mutsuki_plugin_bot_qq_web::QqManagementError,
    > {
        self.service()?.snapshot(query, include_secret_status)
    }

    fn write(
        &self,
        request: mutsuki_plugin_bot_qq_web::QqManagementWriteRequest,
    ) -> Result<
        mutsuki_plugin_bot_qq_web::QqManagementWriteResult,
        mutsuki_plugin_bot_qq_web::QqManagementError,
    > {
        self.service()?.write(request)
    }
}

impl GenerationAwareQqManagement {
    fn service(
        &self,
    ) -> Result<Arc<QqBotManagementService>, mutsuki_plugin_bot_qq_web::QqManagementError> {
        self.runtime
            .host_service(QQ_MANAGEMENT_SERVICE_ID)
            .map_err(|_| mutsuki_plugin_bot_qq_web::QqManagementError {
                code: "qq.owner_unavailable".into(),
                message: "QQ Bot 当前未启用或尚未连接".into(),
            })
    }
}

impl QqConsoleBridge {
    pub fn get(
        runtime: &ServiceRuntime,
    ) -> Option<Arc<dyn mutsuki_plugin_bot_qq_web::QqBotManagementApi>> {
        Some(Arc::new(GenerationAwareQqManagement {
            runtime: runtime.handle(),
        }))
    }
}
