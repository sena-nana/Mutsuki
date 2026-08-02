use std::collections::{HashSet, VecDeque, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

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
    task_account_id: String,
    task_session_digest: u64,
    last_sequence: Option<u64>,
    session_id: Option<String>,
    resume_url: Option<String>,
    seen_dedup_keys: HashSet<Arc<str>>,
    dedup_order: VecDeque<Arc<str>>,
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
        let account_id = account_id.into();
        Self {
            task_account_id: safe_id(&account_id),
            task_session_digest: digest("unidentified"),
            last_sequence: None,
            session_id: None,
            resume_url: None,
            seen_dedup_keys: HashSet::new(),
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
        self.task_session_digest = digest("unidentified");
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

    /// Compact heartbeat JSON without building an intermediate `Value` tree.
    /// Sequence changes are rare on an established connection, so callers may cache the result.
    pub fn heartbeat_text(&self) -> String {
        match self.last_sequence {
            Some(sequence) => format!(r#"{{"op":1,"d":{sequence}}}"#),
            None => r#"{"op":1,"d":null}"#.to_string(),
        }
    }

    pub fn pop_action(&mut self) -> Option<GatewayAction> {
        self.actions.pop_front()
    }

    /// Rolls back a dedup reservation when Host submission rejects the task.
    /// The Gateway may replay the same frame after reconnecting, so retaining
    /// the reservation here would turn temporary Core backpressure into loss.
    pub fn forget_dispatch(&mut self, frame: &GatewayFrame) {
        let key = dedup_key(frame);
        if self.seen_dedup_keys.remove(key.as_str())
            && let Some(index) = self
                .dedup_order
                .iter()
                .position(|item| item.as_ref() == key)
        {
            self.dedup_order.remove(index);
        }
    }

    pub fn handle_raw_frame(
        &mut self,
        raw: Value,
        registry_generation: u64,
    ) -> Result<Option<Task>, String> {
        let object = raw
            .as_object()
            .ok_or_else(|| "invalid_gateway_frame:expected object".to_string())?;
        let op = object
            .get("op")
            .and_then(Value::as_u64)
            .ok_or_else(|| "invalid_gateway_frame:op must be an unsigned integer".to_string())?;
        let sequence = optional_u64(object.get("s"), "s")?;
        if let Some(sequence) = sequence {
            self.last_sequence = Some(sequence);
        }
        match op {
            0 => {
                let event_type = optional_str(object.get("t"), "t")?.unwrap_or("UNKNOWN");
                let event_id = optional_str(object.get("id"), "id")?;
                let data = object.get("d").unwrap_or(&Value::Null);
                if event_type == "READY" {
                    self.session_id = data
                        .get("session_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    self.resume_url = data
                        .get("resume_gateway_url")
                        .or_else(|| data.get("resume_url"))
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    self.task_session_digest =
                        digest(self.session_id.as_deref().unwrap_or("unidentified"));
                }
                if !known_event_type(event_type) {
                    self.actions
                        .push_back(GatewayAction::UnknownEvent(event_type.to_owned()));
                    return Ok(None);
                }
                let key = dedup_key_parts(op, event_type, sequence, event_id, data);
                if self.seen_dedup_keys.contains(key.as_str()) {
                    return Ok(None);
                }
                let task_id = self.task_id(&key);
                let correlation_id = event_id.map(str::to_owned).or_else(|| Some(key.clone()));
                self.remember_dedup_key(key);
                self.actions
                    .push_back(GatewayAction::DispatchTask(task_id.clone()));
                let mut task = Task::new(task_id, QQBOT_GATEWAY_FRAME_PROTOCOL_ID, raw);
                task.registry_generation = registry_generation;
                task.correlation_id = correlation_id;
                Ok(Some(task))
            }
            7 => {
                self.actions.push_back(GatewayAction::Reconnect);
                Ok(None)
            }
            9 => {
                let resumable = object.get("d").and_then(Value::as_bool).unwrap_or(false);
                if resumable && self.session_id.is_some() {
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
            self.task_session_digest = digest(self.session_id.as_deref().unwrap_or("unidentified"));
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
        if self.seen_dedup_keys.contains(key.as_str()) {
            return Ok(None);
        }
        let task_id = self.task_id(&key);
        let correlation_id = frame.id.clone().or_else(|| Some(key.clone()));
        self.remember_dedup_key(key);
        self.actions
            .push_back(GatewayAction::DispatchTask(task_id.clone()));
        let mut task = Task::new(task_id, QQBOT_GATEWAY_FRAME_PROTOCOL_ID, raw);
        task.registry_generation = registry_generation;
        task.correlation_id = correlation_id;
        Ok(Some(task))
    }

    fn remember_dedup_key(&mut self, key: String) {
        let key: Arc<str> = key.into();
        self.seen_dedup_keys.insert(key.clone());
        self.dedup_order.push_back(key);
        while self.dedup_order.len() > self.dedup_window {
            if let Some(expired) = self.dedup_order.pop_front() {
                self.seen_dedup_keys.remove(&expired);
            }
        }
    }

    fn task_id(&self, event_fact: &str) -> String {
        format!(
            "mutsuki.bot.qqbot.gateway.frame:{}:{:016x}:{:016x}",
            self.task_account_id,
            self.task_session_digest,
            digest(event_fact)
        )
    }
}

fn optional_u64(value: Option<&Value>, field: &str) -> Result<Option<u64>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("invalid_gateway_frame:{field} must be an unsigned integer")),
    }
}

fn optional_str<'a>(value: Option<&'a Value>, field: &str) -> Result<Option<&'a str>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(Some)
            .ok_or_else(|| format!("invalid_gateway_frame:{field} must be a string")),
    }
}

fn dedup_key_parts(
    op: u64,
    event_type: &str,
    sequence: Option<u64>,
    id: Option<&str>,
    data: &Value,
) -> String {
    data.get("id")
        .and_then(Value::as_str)
        .map(|id| format!("{event_type}:message:{id}"))
        .or_else(|| id.map(|id| format!("event:{id}")))
        .or_else(|| sequence.map(|sequence| format!("seq:{sequence}")))
        .unwrap_or_else(|| format!("op:{op}:unknown"))
}

pub fn dedup_key(frame: &GatewayFrame) -> String {
    dedup_key_parts(
        frame.op,
        frame.t.as_deref().unwrap_or("UNKNOWN"),
        frame.s,
        frame.id.as_deref(),
        &frame.d,
    )
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
            | "MESSAGE_CREATE"
            | "AT_MESSAGE_CREATE"
            | "DIRECT_MESSAGE_CREATE"
            | "MESSAGE_UPDATE"
            | "MESSAGE_DELETE"
            | "PUBLIC_MESSAGE_DELETE"
            | "DIRECT_MESSAGE_DELETE"
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
            | "GUILD_MEMBER_ADD"
            | "GUILD_MEMBER_UPDATE"
            | "GUILD_MEMBER_REMOVE"
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
