//! Persona store contracts owned by the Bot domain library, not the plugin runner.

#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::collections::BTreeMap;
use std::sync::Mutex;

use mutsuki_bot_protocol::BotPersona;

pub trait PersonaStore: Send + Sync {
    fn upsert(&self, persona: BotPersona) -> Result<(), String>;
    fn list(&self) -> Result<Vec<BotPersona>, String>;
    fn get(&self, persona_id: &str) -> Result<Option<BotPersona>, String>;
    fn bind_conversation(&self, origin_key: &str, persona_id: &str) -> Result<(), String>;
    fn conversation_persona(&self, origin_key: &str) -> Result<Option<String>, String>;
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
