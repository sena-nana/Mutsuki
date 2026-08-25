use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotPersona {
    pub persona_id: String,
    pub name: String,
    pub system_prompt: String,
    #[serde(default)]
    pub begin_dialogs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    pub agent_runtime_profile_id: String,
}

impl BotPersona {
    #[must_use]
    pub fn prompt_text(&self) -> String {
        let mut parts = Vec::new();
        if !self.system_prompt.trim().is_empty() {
            parts.push(self.system_prompt.clone());
        }
        for dialog in &self.begin_dialogs {
            if !dialog.trim().is_empty() {
                parts.push(dialog.clone());
            }
        }
        parts.join("\n")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationIclEntry {
    pub actor_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub text: String,
    pub time_ms: i64,
}

impl ConversationIclEntry {
    #[must_use]
    pub fn speaker(&self) -> &str {
        self.display_name
            .as_deref()
            .filter(|value| !value.is_empty())
            .unwrap_or(self.actor_id.as_str())
    }
}

#[must_use]
pub fn format_icl_summary(entries: &[ConversationIclEntry]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let mut lines = vec!["最近群聊上下文：".into()];
    for entry in entries {
        lines.push(format!("{}: {}", entry.speaker(), entry.text));
    }
    lines.join("\n")
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationIdentifiers {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,
}

impl ConversationIdentifiers {
    #[must_use]
    pub fn prompt_text(&self) -> Option<String> {
        let mut lines = Vec::new();
        if let Some(name) = &self.actor_name {
            lines.push(format!("发言者：{name}"));
        }
        if let Some(name) = &self.group_name {
            lines.push(format!("群：{name}"));
        }
        if let Some(datetime) = &self.datetime {
            lines.push(format!("时间：{datetime}"));
        }
        (!lines.is_empty()).then(|| lines.join("\n"))
    }
}
