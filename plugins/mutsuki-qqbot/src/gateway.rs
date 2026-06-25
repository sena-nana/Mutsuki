use std::collections::{BTreeSet, VecDeque};

use mutsuki_runtime_contracts::{DomainEvent, Task};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::QqBotConfig;
use crate::manifest::RAW_GATEWAY_TASK_KIND;

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
}

#[derive(Clone, Debug)]
pub struct QqGatewayPump {
    next_task_sequence: u64,
    last_sequence: Option<u64>,
    session_id: Option<String>,
    seen_dedup_keys: BTreeSet<String>,
    actions: VecDeque<GatewayAction>,
}

impl Default for QqGatewayPump {
    fn default() -> Self {
        Self::new()
    }
}

impl QqGatewayPump {
    pub fn new() -> Self {
        Self {
            next_task_sequence: 0,
            last_sequence: None,
            session_id: None,
            seen_dedup_keys: BTreeSet::new(),
            actions: VecDeque::new(),
        }
    }

    pub fn last_sequence(&self) -> Option<u64> {
        self.last_sequence
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
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
                "seq": self.last_sequence.unwrap_or(0)
            }
        }))
    }

    pub fn heartbeat_frame(&self) -> Value {
        json!({
            "op": 1,
            "d": self.last_sequence
        })
    }

    pub fn pop_action(&mut self) -> Option<GatewayAction> {
        self.actions.pop_front()
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
            0 => {
                if frame.t.as_deref() == Some("READY") {
                    self.session_id = frame
                        .d
                        .get("session_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
                let key = dedup_key(&frame);
                if !self.seen_dedup_keys.insert(key) {
                    return Ok(None);
                }
                let task_id = self.next_task_id();
                self.actions
                    .push_back(GatewayAction::DispatchTask(task_id.clone()));
                let mut task = Task::new(task_id, RAW_GATEWAY_TASK_KIND, raw);
                task.registry_generation = registry_generation;
                Ok(Some(task))
            }
            7 => {
                self.actions.push_back(GatewayAction::Reconnect);
                Ok(None)
            }
            9 => {
                if frame.d.as_bool().unwrap_or(false) {
                    self.actions.push_back(GatewayAction::Resume);
                } else {
                    self.actions.push_back(GatewayAction::Identify);
                }
                Ok(None)
            }
            10 => {
                self.actions.push_back(GatewayAction::Identify);
                self.actions
                    .push_back(GatewayAction::Heartbeat(self.last_sequence));
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
            _ => Err(format!("unsupported_gateway_op:{}", frame.op)),
        }
    }

    fn next_task_id(&mut self) -> String {
        self.next_task_sequence += 1;
        format!("qqbot.gateway.frame:{}", self.next_task_sequence)
    }
}

pub fn normalize_gateway_frame(frame: GatewayFrame) -> Result<Vec<DomainEvent>, String> {
    if frame.op != 0 {
        return Err(format!("expected_dispatch_op:{}", frame.op));
    }
    let event_type = frame.t.as_deref().unwrap_or("UNKNOWN");
    let kind = match event_type {
        "READY" => "qqbot.gateway.ready",
        "RESUMED" => "qqbot.gateway.resumed",
        "GROUP_MESSAGE_CREATE" | "GROUP_AT_MESSAGE_CREATE" => "qqbot.message.group",
        "C2C_MESSAGE_CREATE" => "qqbot.message.c2c",
        "INTERACTION_CREATE" => "qqbot.interaction",
        "MESSAGE_REACTION_ADD" | "MESSAGE_REACTION_REMOVE" => "qqbot.reaction",
        "FRIEND_ADD"
        | "FRIEND_DEL"
        | "C2C_MSG_REJECT"
        | "C2C_MSG_RECEIVE"
        | "GROUP_ADD_ROBOT"
        | "GROUP_DEL_ROBOT"
        | "GROUP_MSG_REJECT"
        | "GROUP_MSG_RECEIVE"
        | "GROUP_MEMBER_ADD"
        | "GROUP_MEMBER_REMOVE" => "qqbot.lifecycle",
        _ => "qqbot.gateway.unknown",
    };
    let event_id = frame
        .id
        .clone()
        .or_else(|| frame.d.get("id").and_then(Value::as_str).map(str::to_owned))
        .or_else(|| frame.s.map(|sequence| format!("seq:{sequence}")))
        .unwrap_or_else(|| format!("{}:unknown", event_type));
    Ok(vec![DomainEvent {
        event_id,
        kind: kind.into(),
        payload: json!({
            "event_type": event_type,
            "sequence": frame.s,
            "dedup_key": dedup_key(&frame),
            "data": frame.d,
        }),
    }])
}

pub fn dedup_key(frame: &GatewayFrame) -> String {
    frame
        .d
        .get("id")
        .and_then(Value::as_str)
        .map(|id| format!("message:{id}"))
        .or_else(|| frame.id.as_ref().map(|id| format!("event:{id}")))
        .or_else(|| frame.s.map(|sequence| format!("seq:{sequence}")))
        .unwrap_or_else(|| "unknown".into())
}
