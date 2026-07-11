use std::collections::{BTreeSet, VecDeque, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};

use mutsuki_runtime_contracts::Task;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::QqBotConfig;

pub const QQBOT_GATEWAY_FRAME_PROTOCOL_ID: &str = "mutsuki.bot.qqbot.gateway/frame@1";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GatewayFrame {
    pub op: u64,
    #[serde(default)]
    pub s: Option<u64>,
    #[serde(default)]
    pub t: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub d: Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GatewayAction {
    Identify,
    Resume,
    Heartbeat(Option<u64>),
    Reconnect,
    DispatchTask(String),
    AckHeartbeat,
    UnknownOpcode(u64),
    UnknownEvent(String),
}

#[derive(Clone, Debug)]
pub struct QqGatewayPump {
    account_id: String,
    last_sequence: Option<u64>,
    session_id: Option<String>,
    resume_url: Option<String>,
    seen_dedup_keys: BTreeSet<String>,
    dedup_order: VecDeque<String>,
    dedup_window: usize,
    actions: VecDeque<GatewayAction>,
}

impl Default for QqGatewayPump {
    fn default() -> Self {
        Self::new()
    }
}

impl QqGatewayPump {
    pub fn new() -> Self {
        Self::with_account("default", 2_048)
    }

    pub fn with_account(account_id: impl Into<String>, dedup_window: usize) -> Self {
        Self {
            account_id: account_id.into(),
            last_sequence: None,
            session_id: None,
            resume_url: None,
            seen_dedup_keys: BTreeSet::new(),
            dedup_order: VecDeque::new(),
            dedup_window: dedup_window.max(1),
            actions: VecDeque::new(),
        }
    }

    pub fn last_sequence(&self) -> Option<u64> {
        self.last_sequence
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub fn resume_url(&self) -> Option<&str> {
        self.resume_url.as_deref()
    }

    pub fn clear_session(&mut self) {
        self.session_id = None;
        self.resume_url = None;
        self.last_sequence = None;
    }

    pub fn identify_frame(config: &QqBotConfig, access_token: &str) -> Value {
        json!({
            "op": 2,
            "d": {
                "token": format!("QQBot {access_token}"),
                "intents": config.gateway_intents,
                "shard": config.shard,
                "properties": {
                    "$os": "runtime",
                    "$browser": "qqbot",
                    "$device": "qqbot"
                }
            }
        })
    }

    pub fn resume_frame(&self, access_token: &str) -> Result<Value, String> {
        let session_id = self
            .session_id
            .as_deref()
            .ok_or_else(|| "missing_session_id".to_string())?;
        Ok(json!({
            "op": 6,
            "d": {
                "token": format!("QQBot {access_token}"),
                "session_id": session_id,
                "seq": self.last_sequence
            }
        }))
    }

    pub fn heartbeat_frame(&self) -> Value {
        json!({"op": 1, "d": self.last_sequence})
    }

    pub fn pop_action(&mut self) -> Option<GatewayAction> {
        self.actions.pop_front()
    }

    /// Rolls back a dedup reservation when Host submission rejects the task.
    /// The Gateway may replay the same frame after reconnecting, so retaining
    /// the reservation here would turn temporary Core backpressure into loss.
    pub fn forget_dispatch(&mut self, frame: &GatewayFrame) {
        let key = dedup_key(frame);
        if self.seen_dedup_keys.remove(&key)
            && let Some(index) = self.dedup_order.iter().position(|item| item == &key)
        {
            self.dedup_order.remove(index);
        }
    }

    pub fn handle_raw_frame(
        &mut self,
        raw: Value,
        registry_generation: u64,
    ) -> Result<Option<Task>, String> {
        let frame: GatewayFrame = serde_json::from_value(raw.clone())
            .map_err(|error| format!("invalid_gateway_frame:{error}"))?;
        self.handle_frame(frame, raw, registry_generation)
    }

    pub fn handle_frame(
        &mut self,
        frame: GatewayFrame,
        raw: Value,
        registry_generation: u64,
    ) -> Result<Option<Task>, String> {
        if let Some(sequence) = frame.s {
            self.last_sequence = Some(sequence);
        }
        match frame.op {
            0 => self.handle_dispatch(frame, raw, registry_generation),
            7 => {
                self.actions.push_back(GatewayAction::Reconnect);
                Ok(None)
            }
            9 => {
                if frame.d.as_bool().unwrap_or(false) && self.session_id.is_some() {
                    self.actions.push_back(GatewayAction::Resume);
                } else {
                    self.clear_session();
                    self.actions.push_back(GatewayAction::Identify);
                }
                Ok(None)
            }
            10 => {
                self.actions.push_back(if self.session_id.is_some() {
                    GatewayAction::Resume
                } else {
                    GatewayAction::Identify
                });
                Ok(None)
            }
            11 => {
                self.actions.push_back(GatewayAction::AckHeartbeat);
                Ok(None)
            }
            1 => {
                self.actions
                    .push_back(GatewayAction::Heartbeat(self.last_sequence));
                Ok(None)
            }
            opcode => {
                self.actions.push_back(GatewayAction::UnknownOpcode(opcode));
                Ok(None)
            }
        }
    }

    fn handle_dispatch(
        &mut self,
        frame: GatewayFrame,
        raw: Value,
        registry_generation: u64,
    ) -> Result<Option<Task>, String> {
        let event_type = frame.t.as_deref().unwrap_or("UNKNOWN");
        if event_type == "READY" {
            self.session_id = frame
                .d
                .get("session_id")
                .and_then(Value::as_str)
                .map(str::to_owned);
            self.resume_url = frame
                .d
                .get("resume_gateway_url")
                .or_else(|| frame.d.get("resume_url"))
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        if !known_event_type(event_type) {
            self.actions
                .push_back(GatewayAction::UnknownEvent(event_type.to_owned()));
            return Ok(None);
        }
        let key = dedup_key(&frame);
        if self.seen_dedup_keys.contains(&key) {
            return Ok(None);
        }
        self.remember_dedup_key(key.clone());
        let task_id = self.task_id(&key);
        self.actions
            .push_back(GatewayAction::DispatchTask(task_id.clone()));
        let mut task = Task::new(task_id, QQBOT_GATEWAY_FRAME_PROTOCOL_ID, raw);
        task.registry_generation = registry_generation;
        task.correlation_id = frame.id.clone().or(Some(key));
        Ok(Some(task))
    }

    fn remember_dedup_key(&mut self, key: String) {
        self.seen_dedup_keys.insert(key.clone());
        self.dedup_order.push_back(key);
        while self.dedup_order.len() > self.dedup_window {
            if let Some(expired) = self.dedup_order.pop_front() {
                self.seen_dedup_keys.remove(&expired);
            }
        }
    }

    fn task_id(&self, event_fact: &str) -> String {
        let session = self.session_id.as_deref().unwrap_or("unidentified");
        format!(
            "mutsuki.bot.qqbot.gateway.frame:{}:{:016x}:{:016x}",
            safe_id(&self.account_id),
            digest(session),
            digest(event_fact)
        )
    }
}

pub fn dedup_key(frame: &GatewayFrame) -> String {
    frame
        .d
        .get("id")
        .and_then(Value::as_str)
        .map(|id| format!("message:{id}"))
        .or_else(|| frame.id.as_ref().map(|id| format!("event:{id}")))
        .or_else(|| frame.s.map(|sequence| format!("seq:{sequence}")))
        .unwrap_or_else(|| format!("op:{}:unknown", frame.op))
}

pub fn session_summary(session_id: Option<&str>) -> String {
    session_id
        .map(|session| format!("{:08x}", digest(session) as u32))
        .unwrap_or_else(|| "none".into())
}

fn known_event_type(event_type: &str) -> bool {
    matches!(
        event_type,
        "READY"
            | "RESUMED"
            | "GROUP_MESSAGE_CREATE"
            | "GROUP_AT_MESSAGE_CREATE"
            | "C2C_MESSAGE_CREATE"
            | "INTERACTION_CREATE"
            | "FRIEND_ADD"
            | "FRIEND_DEL"
            | "C2C_MSG_REJECT"
            | "C2C_MSG_RECEIVE"
            | "GROUP_ADD_ROBOT"
            | "GROUP_DEL_ROBOT"
            | "GROUP_MSG_REJECT"
            | "GROUP_MSG_RECEIVE"
            | "GROUP_MEMBER_ADD"
            | "GROUP_MEMBER_REMOVE"
            | "MESSAGE_REACTION_ADD"
            | "MESSAGE_REACTION_REMOVE"
    )
}

fn digest(value: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn safe_id(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .take(48)
        .collect()
}
