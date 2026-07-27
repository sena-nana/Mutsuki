//! Process-local bridge from Bilibili configured-plugin prepare to the embedded Web Console.

use std::sync::{Arc, OnceLock, RwLock};

use mutsuki_plugin_bot_bilibili::BilibiliManagementService;

static BRIDGE: OnceLock<RwLock<Option<Arc<BilibiliManagementService>>>> = OnceLock::new();

fn slot() -> &'static RwLock<Option<Arc<BilibiliManagementService>>> {
    BRIDGE.get_or_init(|| RwLock::new(None))
}

pub struct BilibiliConsoleBridge;

impl BilibiliConsoleBridge {
    pub fn clear() {
        *slot().write().expect("bilibili console bridge write") = None;
    }

    pub fn publish(service: Arc<BilibiliManagementService>) {
        *slot().write().expect("bilibili console bridge write") = Some(service);
    }

    pub fn get() -> Option<Arc<BilibiliManagementService>> {
        slot().read().expect("bilibili console bridge read").clone()
    }
}
