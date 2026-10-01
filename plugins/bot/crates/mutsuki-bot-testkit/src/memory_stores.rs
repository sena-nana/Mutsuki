//! In-memory store doubles for tests.
//!
//! These used to sit in the production library crates next to the traits they
//! implement, publicly exported and re-exported again by the plugin crates, even
//! though the only implementation any product uses is `BotStateDbRepository`. Keeping
//! them here makes it impossible to reach for one by accident from production code.

use std::collections::BTreeMap;
use std::sync::Mutex;

use mutsuki_bot_conversation::{ConversationContextStore, PersonaStore};
use mutsuki_bot_protocol::{BotPersona, ConversationIclEntry};

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

#[derive(Default)]
pub struct MemoryPersonaStore {
    personas: Mutex<BTreeMap<String, BotPersona>>,
    bindings: Mutex<BTreeMap<String, String>>,
}

impl PersonaStore for MemoryPersonaStore {
    fn upsert(&self, persona: BotPersona) -> Result<(), String> {
        self.personas
            .lock()
            .map_err(|error| error.to_string())?
            .insert(persona.persona_id.clone(), persona);
        Ok(())
    }

    fn list(&self) -> Result<Vec<BotPersona>, String> {
        Ok(self
            .personas
            .lock()
            .map_err(|error| error.to_string())?
            .values()
            .cloned()
            .collect())
    }

    fn get(&self, persona_id: &str) -> Result<Option<BotPersona>, String> {
        Ok(self
            .personas
            .lock()
            .map_err(|error| error.to_string())?
            .get(persona_id)
            .cloned())
    }

    fn bind_conversation(&self, origin_key: &str, persona_id: &str) -> Result<(), String> {
        if self.get(persona_id)?.is_none() {
            return Err(format!("unknown persona {persona_id}"));
        }
        self.bindings
            .lock()
            .map_err(|error| error.to_string())?
            .insert(origin_key.to_owned(), persona_id.to_owned());
        Ok(())
    }

    fn conversation_persona(&self, origin_key: &str) -> Result<Option<String>, String> {
        Ok(self
            .bindings
            .lock()
            .map_err(|error| error.to_string())?
            .get(origin_key)
            .cloned())
    }
}
