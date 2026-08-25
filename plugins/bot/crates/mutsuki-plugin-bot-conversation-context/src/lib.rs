// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate
)]

use std::collections::BTreeMap;
use std::sync::Arc;

use mutsuki_bot_protocol::{
    BOT_CONVERSATION_ATTACH_ICL_PROTOCOL_ID, BOT_CONVERSATION_ATTACH_IDENTIFIERS_PROTOCOL_ID,
    BOT_CONVERSATION_RECORD_ICL_PROTOCOL_ID, BOT_EXT_CONVERSATION_ICL,
    BOT_EXT_CONVERSATION_IDENTIFIERS, BOT_FLOW_BOT_EVENT_TYPE, BotEvent, BotFlowTypeRef,
    BotNodeBinding, BotNodeCatalogFragment, BotNodeDescriptor, BotNodeInvocation, BotNodeOutput,
    BotNodePortDescriptor, BotNodePortDirection, BotNodeResult, BotNodeRole, BotTarget,
    ConversationIclEntry, ConversationIdentifiers,
};
use mutsuki_runtime_contracts::{
    CompletionBatch, ExecutionClass, PluginManifest, RunnerDescriptor, RunnerResult, Task,
    WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeResult};
use mutsuki_runtime_sdk::{
    PluginBuilder, ProtocolDescriptorBuilder, RunnerDescriptorBuilder, map_work_batch_entries,
};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};

pub const BOT_CONVERSATION_CONTEXT_PLUGIN_ID: &str = "mutsuki.plugin.bot.conversation.context";
pub const BOT_CONVERSATION_CONTEXT_RUNNER_ID: &str = "mutsuki.bot.conversation.context";
pub const BOT_CONVERSATION_RECORD_ICL_NODE_TYPE: &str = "mutsuki.bot.conversation.record_icl";
pub const BOT_CONVERSATION_ATTACH_ICL_NODE_TYPE: &str = "mutsuki.bot.conversation.attach_icl";
pub const BOT_CONVERSATION_ATTACH_IDENTIFIERS_NODE_TYPE: &str =
    "mutsuki.bot.conversation.attach_identifiers";

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
        let mut entries = self.entries.lock();
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
        let entries = self.entries.lock();
        let list = entries.get(origin_key).cloned().unwrap_or_default();
        if max_count == 0 || list.len() <= max_count {
            Ok(list)
        } else {
            Ok(list[list.len() - max_count..].to_vec())
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct IclConfig {
    max_count: usize,
}

impl Default for IclConfig {
    fn default() -> Self {
        Self { max_count: 20 }
    }
}

#[must_use]
pub fn bot_conversation_context_manifest() -> PluginManifest {
    PluginBuilder::new(BOT_CONVERSATION_CONTEXT_PLUGIN_ID)
        .runner_descriptor(context_descriptor())
        .protocol_handler(
            node_protocol(BOT_CONVERSATION_RECORD_ICL_PROTOCOL_ID),
            BOT_CONVERSATION_CONTEXT_RUNNER_ID,
            "record-icl",
        )
        .protocol_handler(
            node_protocol(BOT_CONVERSATION_ATTACH_ICL_PROTOCOL_ID),
            BOT_CONVERSATION_CONTEXT_RUNNER_ID,
            "attach-icl",
        )
        .protocol_handler(
            node_protocol(BOT_CONVERSATION_ATTACH_IDENTIFIERS_PROTOCOL_ID),
            BOT_CONVERSATION_CONTEXT_RUNNER_ID,
            "attach-identifiers",
        )
        .extension(
            conversation_context_catalog()
                .into_plugin_extension()
                .expect("conversation context catalog serializes"),
        )
        .build()
        .manifest
}

fn node_protocol(protocol_id: &str) -> mutsuki_runtime_contracts::ProtocolDescriptor {
    ProtocolDescriptorBuilder::new(protocol_id)
        .input_schema(json!({"type": "object"}))
        .output_schema(json!({"type": "object", "required": ["outputs", "metadata"]}))
        .error_schema(json!({"type": "object", "required": ["code", "source", "route"]}))
        .build()
}

fn conversation_context_catalog() -> BotNodeCatalogFragment {
    BotNodeCatalogFragment {
        nodes: vec![
            processor_node(
                BOT_CONVERSATION_RECORD_ICL_NODE_TYPE,
                "记录群聊旁听",
                BOT_CONVERSATION_RECORD_ICL_PROTOCOL_ID,
                json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "max_count": {"type": "integer", "minimum": 1, "default": 20, "title": "保留条数"}
                    }
                }),
            ),
            processor_node(
                BOT_CONVERSATION_ATTACH_ICL_NODE_TYPE,
                "附加群聊上下文",
                BOT_CONVERSATION_ATTACH_ICL_PROTOCOL_ID,
                json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "max_count": {"type": "integer", "minimum": 1, "default": 20, "title": "附加条数"}
                    }
                }),
            ),
            processor_node(
                BOT_CONVERSATION_ATTACH_IDENTIFIERS_NODE_TYPE,
                "附加会话标识",
                BOT_CONVERSATION_ATTACH_IDENTIFIERS_PROTOCOL_ID,
                json!({"type": "object", "additionalProperties": false}),
            ),
        ],
    }
}

fn processor_node(
    node_type_id: &str,
    title: &str,
    protocol_id: &str,
    config_schema: Value,
) -> BotNodeDescriptor {
    BotNodeDescriptor {
        node_type_id: node_type_id.into(),
        version: 1,
        title: title.into(),
        category: "会话".into(),
        role: BotNodeRole::Processor,
        binding: Some(BotNodeBinding {
            binding_id: format!("binding:{protocol_id}"),
            protocol_id: protocol_id.into(),
            runner_hint: Some(BOT_CONVERSATION_CONTEXT_RUNNER_ID.into()),
        }),
        ports: vec![
            BotNodePortDescriptor {
                port_id: "input".into(),
                title: "事件".into(),
                direction: BotNodePortDirection::Input,
                event_type: BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1),
                required: true,
            },
            BotNodePortDescriptor {
                port_id: "output".into(),
                title: "事件".into(),
                direction: BotNodePortDirection::Output,
                event_type: BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1),
                required: false,
            },
        ],
        config_schema,
    }
}

pub struct ConversationContextRunner {
    store: Arc<dyn ConversationContextStore>,
    descriptor: RunnerDescriptor,
}

impl ConversationContextRunner {
    pub fn new(store: Arc<dyn ConversationContextStore>) -> Self {
        Self {
            store,
            descriptor: context_descriptor(),
        }
    }
}

impl Runner for ConversationContextRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, |task| {
            let invocation = task
                .payload
                .decode_shared::<BotNodeInvocation>()
                .map_err(|error| runtime_error(task, error))?;
            let mut event: BotEvent =
                serde_json::from_value(invocation.input.payload.value.clone())
                    .map_err(|error| runtime_error(task, error))?;
            match task.protocol_id.as_str() {
                BOT_CONVERSATION_RECORD_ICL_PROTOCOL_ID => {
                    record_icl(self.store.as_ref(), &event, &invocation.config)
                        .map_err(|error| runtime_error(task, error))?;
                }
                BOT_CONVERSATION_ATTACH_ICL_PROTOCOL_ID => {
                    attach_icl(self.store.as_ref(), &mut event, &invocation.config)
                        .map_err(|error| runtime_error(task, error))?;
                }
                BOT_CONVERSATION_ATTACH_IDENTIFIERS_PROTOCOL_ID => {
                    attach_identifiers(&mut event);
                }
                other => return Err(runtime_error(task, other)),
            }
            completed_event(task, &invocation, event)
        })
    }
}

fn record_icl(
    store: &dyn ConversationContextStore,
    event: &BotEvent,
    config: &Value,
) -> Result<(), String> {
    let config: IclConfig =
        serde_json::from_value(config.clone()).map_err(|error| error.to_string())?;
    let Some(entry) = icl_entry(event) else {
        return Ok(());
    };
    store.record_icl(&event.target.conversation_key(), entry, config.max_count)
}

fn attach_icl(
    store: &dyn ConversationContextStore,
    event: &mut BotEvent,
    config: &Value,
) -> Result<(), String> {
    let config: IclConfig =
        serde_json::from_value(config.clone()).map_err(|error| error.to_string())?;
    let entries = store.load_icl(&event.target.conversation_key(), config.max_count)?;
    event.ext.insert(
        BOT_EXT_CONVERSATION_ICL.into(),
        serde_json::to_value(entries).map_err(|error| error.to_string())?,
    );
    Ok(())
}

fn attach_identifiers(event: &mut BotEvent) {
    let identifiers = ConversationIdentifiers {
        actor_name: event
            .actor
            .as_ref()
            .and_then(|actor| actor.display_name.clone()),
        group_name: event
            .ext
            .get("qqbot.group_name")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| match &event.target {
                BotTarget::Group { group_id } => Some(group_id.clone()),
                _ => None,
            }),
        datetime: Some(format!("unix_ms:{}", event.time_ms)),
    };
    if let Ok(value) = serde_json::to_value(identifiers) {
        event
            .ext
            .insert(BOT_EXT_CONVERSATION_IDENTIFIERS.into(), value);
    }
}

fn icl_entry(event: &BotEvent) -> Option<ConversationIclEntry> {
    let text = event.message.as_ref()?.plain_text();
    if text.chars().all(char::is_whitespace) {
        return None;
    }
    Some(ConversationIclEntry {
        actor_id: event.actor.as_ref()?.user_id.clone(),
        display_name: event
            .actor
            .as_ref()
            .and_then(|actor| actor.display_name.clone()),
        text,
        time_ms: event.time_ms,
    })
}

fn completed_event(
    task: &Task,
    invocation: &BotNodeInvocation,
    event: BotEvent,
) -> Result<RunnerResult, mutsuki_runtime_contracts::RuntimeError> {
    let mut output_event = invocation.input.clone();
    output_event.payload.value =
        serde_json::to_value(&event).map_err(|error| runtime_error(task, error))?;
    let mut result = RunnerResult::completed(task.task_id.clone());
    result.output = Some(
        serde_json::to_value(BotNodeResult {
            outputs: vec![BotNodeOutput {
                port_id: "output".into(),
                event: output_event,
            }],
            metadata: BTreeMap::new(),
        })
        .map_err(|error| runtime_error(task, error))?,
    );
    Ok(result)
}

fn context_descriptor() -> RunnerDescriptor {
    RunnerDescriptorBuilder::new(
        BOT_CONVERSATION_CONTEXT_RUNNER_ID,
        BOT_CONVERSATION_CONTEXT_PLUGIN_ID,
    )
    .accepted_protocol(BOT_CONVERSATION_RECORD_ICL_PROTOCOL_ID)
    .accepted_protocol(BOT_CONVERSATION_ATTACH_ICL_PROTOCOL_ID)
    .accepted_protocol(BOT_CONVERSATION_ATTACH_IDENTIFIERS_PROTOCOL_ID)
    .execution_class(ExecutionClass::Orchestration)
    .build()
}

fn runtime_error(
    task: &Task,
    error: impl std::fmt::Display,
) -> mutsuki_runtime_contracts::RuntimeError {
    let mut value = mutsuki_runtime_contracts::RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        BOT_CONVERSATION_CONTEXT_PLUGIN_ID,
        format!("{}.{}", task.task_id, task.protocol_id),
    );
    value.evidence.insert(
        "message".into(),
        mutsuki_runtime_contracts::ScalarValue::String(error.to_string()),
    );
    value
}

#[cfg(test)]
mod tests {
    use mutsuki_bot_protocol::{
        BotAccountRef, BotEventKind, BotMessage, BotPlatform, BotUser, format_icl_summary,
    };

    use super::*;

    fn event(text: &str) -> BotEvent {
        BotEvent {
            event_id: "e1".into(),
            platform: BotPlatform::QqBot,
            bot: BotAccountRef {
                account_id: "bot".into(),
                platform: BotPlatform::QqBot,
            },
            kind: BotEventKind::MessageCreated,
            time_ms: 1,
            target: BotTarget::Group {
                group_id: "g1".into(),
            },
            actor: Some(BotUser {
                user_id: "u1".into(),
                display_name: Some("Alice".into()),
                avatar_url: None,
            }),
            message: Some(BotMessage::text(
                BotTarget::Group {
                    group_id: "g1".into(),
                },
                text,
            )),
            raw: None,
            ext: Default::default(),
        }
    }

    #[test]
    fn record_and_attach_icl_keeps_recent_messages() {
        let store = Arc::new(MemoryConversationContextStore::default());
        let config = json!({"max_count": 2});
        record_icl(store.as_ref(), &event("one"), &config).unwrap();
        record_icl(store.as_ref(), &event("two"), &config).unwrap();
        record_icl(store.as_ref(), &event("three"), &config).unwrap();
        let mut attached = event("ask");
        attach_icl(store.as_ref(), &mut attached, &config).unwrap();
        let entries: Vec<ConversationIclEntry> =
            serde_json::from_value(attached.ext[BOT_EXT_CONVERSATION_ICL].clone()).unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.text.as_str())
                .collect::<Vec<_>>(),
            ["two", "three"]
        );
        assert!(format_icl_summary(&entries).contains("Alice: two"));
    }

    #[test]
    fn identifiers_capture_actor_and_group() {
        let mut event = event("hi");
        event.ext.insert("qqbot.group_name".into(), json!("测试群"));
        attach_identifiers(&mut event);
        let identifiers: ConversationIdentifiers =
            serde_json::from_value(event.ext[BOT_EXT_CONVERSATION_IDENTIFIERS].clone()).unwrap();
        assert_eq!(identifiers.actor_name.as_deref(), Some("Alice"));
        assert_eq!(identifiers.group_name.as_deref(), Some("测试群"));
        assert!(identifiers.datetime.is_some());
    }
}
