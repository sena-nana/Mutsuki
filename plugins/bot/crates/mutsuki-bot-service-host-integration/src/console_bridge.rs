//! Runtime-local bridge from Bilibili configured-plugin services to the embedded Web Console.

use std::sync::Arc;

use mutsuki_plugin_bot_agent::{BOT_AGENT_CONFIG_SERVICE_ID, BotAgentConfigHandle};
use mutsuki_plugin_bot_bilibili::BilibiliManagementService;
use mutsuki_service_runtime::ServiceRuntime;

pub const BILIBILI_MANAGEMENT_SERVICE_ID: &str = "mutsuki.bot.bilibili.management";

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
