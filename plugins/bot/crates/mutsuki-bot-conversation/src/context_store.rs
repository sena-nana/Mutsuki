use std::collections::BTreeMap;
use std::sync::Mutex;

use mutsuki_bot_protocol::ConversationIclEntry;

pub trait ConversationContextStore: Send + Sync {
    fn record_icl(
        &self,
        origin_key: &str,
        entry: ConversationIclEntry,
        max_count: usize,
    ) -> Result<(), String>;
    fn load_icl(
        &self,
        origin_key: &str,
        max_count: usize,
    ) -> Result<Vec<ConversationIclEntry>, String>;
}

#[derive(Default)]
pub struct MemoryConversationContextStore {
    entries: Mutex<BTreeMap<String, Vec<ConversationIclEntry>>>,
}

impl ConversationContextStore for MemoryConversationContextStore {
    fn record_icl(
        &self,
        origin_key: &str,
        entry: ConversationIclEntry,
        max_count: usize,
    ) -> Result<(), String> {
        let mut entries = self.entries.lock().map_err(|error| error.to_string())?;
        let list = entries.entry(origin_key.to_owned()).or_default();
        list.push(entry);
        if max_count > 0 && list.len() > max_count {
            let extra = list.len() - max_count;
            list.drain(..extra);
        }
        Ok(())
    }

    fn load_icl(
        &self,
        origin_key: &str,
        max_count: usize,
    ) -> Result<Vec<ConversationIclEntry>, String> {
        let entries = self.entries.lock().map_err(|error| error.to_string())?;
        let list = entries.get(origin_key).cloned().unwrap_or_default();
        if max_count == 0 || list.len() <= max_count {
            Ok(list)
        } else {
            Ok(list[list.len() - max_count..].to_vec())
        }
    }
}
