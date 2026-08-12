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
use mutsuki_service_runtime::ServiceRuntime;

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
    pub fn get(runtime: &ServiceRuntime) -> Option<Arc<AgentConnectionManager>> {
        runtime
            .host_service(AGENT_CONNECTION_MANAGEMENT_SERVICE_ID)
            .ok()
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

impl QqConsoleBridge {
    pub fn get(
        runtime: &ServiceRuntime,
    ) -> Option<Arc<dyn mutsuki_plugin_bot_qq_web::QqBotManagementApi>> {
        runtime
            .host_service::<QqBotManagementService>(QQ_MANAGEMENT_SERVICE_ID)
            .ok()
            .map(|service| service as Arc<dyn mutsuki_plugin_bot_qq_web::QqBotManagementApi>)
    }
}
