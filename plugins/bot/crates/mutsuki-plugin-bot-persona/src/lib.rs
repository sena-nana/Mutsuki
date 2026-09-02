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
    BOT_AGENT_ATTACH_BOUND_PERSONA_PROTOCOL_ID, BOT_AGENT_PERSONA_PROTOCOL_ID,
    BOT_COMMAND_REPLY_PROTOCOL_ID, BOT_EXT_AGENT_PROFILE_ID, BOT_EXT_PERSONA_PROMPT,
    BOT_FLOW_BOT_EVENT_TYPE, BotCommandEvent, BotEvent, BotFlowTypeRef, BotMessage, BotNodeBinding,
    BotNodeCatalogFragment, BotNodeDescriptor, BotNodeInvocation, BotNodeOutput,
    BotNodePortDescriptor, BotNodePortDirection, BotNodeResult, BotNodeRole,
};
use mutsuki_runtime_contracts::{
    CompletionBatch, ExecutionClass, PluginManifest, RunnerDescriptor, RunnerResult, Task,
    WorkBatch,
};
use mutsuki_runtime_core::{Runner, RunnerContext, RuntimeResult};
use mutsuki_runtime_sdk::{
    PluginBuilder, ProtocolDescriptorBuilder, RunnerDescriptorBuilder, map_work_batch_entries,
};
use serde_json::json;

pub use mutsuki_bot_persona::{MemoryPersonaStore, PersonaStore};

pub const BOT_PERSONA_PLUGIN_ID: &str = "mutsuki.plugin.bot.persona";
pub const BOT_PERSONA_RUNNER_ID: &str = "mutsuki.bot.persona";
pub const BOT_PERSONA_COMMAND_NODE_TYPE: &str = "mutsuki.bot.agent.persona";
pub const BOT_PERSONA_ATTACH_NODE_TYPE: &str = "mutsuki.bot.agent.attach_bound_persona";

#[must_use]
pub fn bot_persona_manifest() -> PluginManifest {
    PluginBuilder::new(BOT_PERSONA_PLUGIN_ID)
        .runner_descriptor(persona_descriptor())
        .protocol_handler(
            ProtocolDescriptorBuilder::new(BOT_AGENT_PERSONA_PROTOCOL_ID)
                .input_schema(json!({"type": "object"}))
                .output_schema(json!({"type": "object", "required": ["outputs", "metadata"]}))
                .error_schema(json!({"type": "object", "required": ["code", "source", "route"]}))
                .build(),
            BOT_PERSONA_RUNNER_ID,
            "persona",
        )
        .protocol_handler(
            ProtocolDescriptorBuilder::new(BOT_AGENT_ATTACH_BOUND_PERSONA_PROTOCOL_ID)
                .input_schema(json!({"type": "object"}))
                .output_schema(json!({"type": "object", "required": ["outputs", "metadata"]}))
                .error_schema(json!({"type": "object", "required": ["code", "source", "route"]}))
                .build(),
            BOT_PERSONA_RUNNER_ID,
            "attach-bound-persona",
        )
        .extension(
            persona_catalog()
                .into_plugin_extension()
                .expect("persona catalog serializes"),
        )
        .build()
        .manifest
}

fn persona_catalog() -> BotNodeCatalogFragment {
    BotNodeCatalogFragment {
        nodes: vec![
            BotNodeDescriptor {
                node_type_id: BOT_PERSONA_COMMAND_NODE_TYPE.into(),
                version: 1,
                title: "人格命令".into(),
                category: "Agent".into(),
                role: BotNodeRole::Processor,
                binding: Some(BotNodeBinding {
                    binding_id: format!("binding:{BOT_AGENT_PERSONA_PROTOCOL_ID}"),
                    protocol_id: BOT_AGENT_PERSONA_PROTOCOL_ID.into(),
                    runner_hint: Some(BOT_PERSONA_RUNNER_ID.into()),
                }),
                ports: vec![
                    BotNodePortDescriptor {
                        port_id: "command".into(),
                        title: "命令".into(),
                        direction: BotNodePortDirection::Input,
                        event_type: BotFlowTypeRef::new("mutsuki.bot.command.event", 1),
                        required: true,
                    },
                    BotNodePortDescriptor {
                        port_id: "message".into(),
                        title: "回复".into(),
                        direction: BotNodePortDirection::Output,
                        event_type: BotFlowTypeRef::new("mutsuki.bot.message.send", 1),
                        required: false,
                    },
                    BotNodePortDescriptor {
                        port_id: "event".into(),
                        title: "事件".into(),
                        direction: BotNodePortDirection::Output,
                        event_type: BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1),
                        required: false,
                    },
                ],
                config_schema: json!({"type": "object", "additionalProperties": false}),
            },
            BotNodeDescriptor {
                node_type_id: BOT_PERSONA_ATTACH_NODE_TYPE.into(),
                version: 1,
                title: "附加已绑定人格".into(),
                category: "Agent".into(),
                role: BotNodeRole::Processor,
                binding: Some(BotNodeBinding {
                    binding_id: format!("binding:{BOT_AGENT_ATTACH_BOUND_PERSONA_PROTOCOL_ID}"),
                    protocol_id: BOT_AGENT_ATTACH_BOUND_PERSONA_PROTOCOL_ID.into(),
                    runner_hint: Some(BOT_PERSONA_RUNNER_ID.into()),
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
                config_schema: json!({"type": "object", "additionalProperties": false}),
            },
        ],
    }
}

pub struct PersonaRunner {
    store: Arc<dyn PersonaStore>,
    descriptor: RunnerDescriptor,
}

impl PersonaRunner {
    pub fn new(store: Arc<dyn PersonaStore>) -> Self {
        Self {
            store,
            descriptor: persona_descriptor(),
        }
    }
}

fn persona_descriptor() -> RunnerDescriptor {
    RunnerDescriptorBuilder::new(BOT_PERSONA_RUNNER_ID, BOT_PERSONA_PLUGIN_ID)
        .accepted_protocol(BOT_AGENT_PERSONA_PROTOCOL_ID)
        .accepted_protocol(BOT_AGENT_ATTACH_BOUND_PERSONA_PROTOCOL_ID)
        .execution_class(ExecutionClass::Orchestration)
        .build()
}

impl Runner for PersonaRunner {
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
            if task.protocol_id.as_str() == BOT_AGENT_ATTACH_BOUND_PERSONA_PROTOCOL_ID {
                let mut event: BotEvent =
                    serde_json::from_value(invocation.input.payload.value.clone())
                        .map_err(|error| runtime_error(task, error))?;
                attach_bound_persona(self.store.as_ref(), &mut event)
                    .map_err(|error| runtime_error(task, error))?;
                completed_event(task, &invocation, event)
            } else {
                let command: BotCommandEvent =
                    serde_json::from_value(invocation.input.payload.value.clone())
                        .map_err(|error| runtime_error(task, error))?;
                let (text, event) = handle_persona(self.store.as_ref(), command)
                    .map_err(|error| runtime_error(task, error))?;
                completed(task, &invocation, text, event)
            }
        })
    }
}

fn handle_persona(
    store: &dyn PersonaStore,
    mut command: BotCommandEvent,
) -> Result<(String, BotEvent), String> {
    let origin = command.source.target.conversation_key();
    if command.args.is_empty() {
        let personas = store.list()?;
        let current = store.conversation_persona(&origin)?;
        let mut lines = vec!["可用人格：".into()];
        for persona in personas {
            let marker = if current.as_deref() == Some(persona.persona_id.as_str()) {
                "*"
            } else {
                " "
            };
            lines.push(format!(
                "{marker} {} ({})",
                persona.name, persona.persona_id
            ));
        }
        return Ok((lines.join("\n"), command.source));
    }
    let persona_id = command.args[0].clone();
    let persona = store
        .get(&persona_id)?
        .ok_or_else(|| format!("unknown persona {persona_id}"))?;
    store.bind_conversation(&origin, &persona_id)?;
    command.source.ext.insert(
        BOT_EXT_AGENT_PROFILE_ID.into(),
        json!(persona.agent_runtime_profile_id),
    );
    command
        .source
        .ext
        .insert(BOT_EXT_PERSONA_PROMPT.into(), json!(persona.prompt_text()));
    Ok((format!("已切换人格 {}", persona.name), command.source))
}

fn attach_bound_persona(store: &dyn PersonaStore, event: &mut BotEvent) -> Result<(), String> {
    let Some(persona_id) = store.conversation_persona(&event.target.conversation_key())? else {
        return Ok(());
    };
    let Some(persona) = store.get(&persona_id)? else {
        return Ok(());
    };
    event.ext.insert(
        BOT_EXT_AGENT_PROFILE_ID.into(),
        json!(persona.agent_runtime_profile_id),
    );
    event
        .ext
        .insert(BOT_EXT_PERSONA_PROMPT.into(), json!(persona.prompt_text()));
    Ok(())
}

#[allow(clippy::needless_pass_by_value)]
fn completed(
    task: &Task,
    invocation: &BotNodeInvocation,
    text: String,
    event: BotEvent,
) -> Result<RunnerResult, mutsuki_runtime_contracts::RuntimeError> {
    let message = BotMessage::text(event.target.clone(), text);
    let mut event_out = invocation.input.clone();
    event_out.protocol_id = BOT_FLOW_BOT_EVENT_TYPE.into();
    event_out.payload.event_type = BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1);
    event_out.payload.value =
        serde_json::to_value(&event).map_err(|error| runtime_error(task, error))?;
    let mut message_out = invocation.input.clone();
    message_out.protocol_id = BOT_COMMAND_REPLY_PROTOCOL_ID.into();
    message_out.payload.event_type = BotFlowTypeRef::new("mutsuki.bot.message.send", 1);
    message_out.payload.value =
        serde_json::to_value(message).map_err(|error| runtime_error(task, error))?;
    let mut result = RunnerResult::completed(task.task_id.clone());
    result.output = Some(
        serde_json::to_value(BotNodeResult {
            outputs: vec![
                BotNodeOutput {
                    port_id: "message".into(),
                    event: message_out,
                },
                BotNodeOutput {
                    port_id: "event".into(),
                    event: event_out,
                },
            ],
            metadata: BTreeMap::new(),
        })
        .map_err(|error| runtime_error(task, error))?,
    );
    Ok(result)
}

#[allow(clippy::needless_pass_by_value)]
fn completed_event(
    task: &Task,
    invocation: &BotNodeInvocation,
    event: BotEvent,
) -> Result<RunnerResult, mutsuki_runtime_contracts::RuntimeError> {
    let mut output_event = invocation.input.clone();
    output_event.protocol_id = BOT_FLOW_BOT_EVENT_TYPE.into();
    output_event.payload.event_type = BotFlowTypeRef::new(BOT_FLOW_BOT_EVENT_TYPE, 1);
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

fn runtime_error(
    task: &Task,
    error: impl std::fmt::Display,
) -> mutsuki_runtime_contracts::RuntimeError {
    let mut value = mutsuki_runtime_contracts::RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        BOT_PERSONA_PLUGIN_ID,
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
    use mutsuki_bot_protocol::{BotPersona, BotTarget};

    use super::*;

    fn persona() -> BotPersona {
        BotPersona {
            persona_id: "guide".into(),
            name: "向导".into(),
            system_prompt: "你是向导。".into(),
            begin_dialogs: vec!["先自我介绍".into()],
            allowed_tools: None,
            agent_runtime_profile_id: "qq-guide".into(),
        }
    }

    #[test]
    fn persona_prompt_joins_system_and_begin_dialogs() {
        assert_eq!(persona().prompt_text(), "你是向导。\n先自我介绍");
    }

    #[test]
    fn switch_binds_conversation_and_writes_profile_ext() {
        let store = Arc::new(MemoryPersonaStore::default());
        store.upsert(persona()).unwrap();
        let event = BotEvent {
            event_id: "e".into(),
            platform: mutsuki_bot_protocol::BotPlatform::QqBot,
            bot: mutsuki_bot_protocol::BotAccountRef {
                account_id: "bot".into(),
                platform: mutsuki_bot_protocol::BotPlatform::QqBot,
            },
            kind: mutsuki_bot_protocol::BotEventKind::MessageCreated,
            time_ms: 1,
            target: BotTarget::Group {
                group_id: "g1".into(),
            },
            actor: None,
            message: None,
            raw: None,
            ext: BTreeMap::default(),
        };
        let (_, event) = handle_persona(
            store.as_ref(),
            BotCommandEvent {
                source: event,
                name: "persona".into(),
                args: vec!["guide".into()],
                command_path: vec!["persona".into()],
                typed_args: BTreeMap::new(),
                raw_text: "/persona guide".into(),
            },
        )
        .unwrap();
        assert_eq!(
            event.ext[BOT_EXT_AGENT_PROFILE_ID].as_str(),
            Some("qq-guide")
        );
        assert_eq!(
            store.conversation_persona("group:g1").unwrap().as_deref(),
            Some("guide")
        );
    }

    #[test]
    fn attach_bound_persona_writes_profile_ext_from_store() {
        let store = Arc::new(MemoryPersonaStore::default());
        store.upsert(persona()).unwrap();
        store.bind_conversation("group:g1", "guide").unwrap();
        let mut event = BotEvent {
            event_id: "e".into(),
            platform: mutsuki_bot_protocol::BotPlatform::QqBot,
            bot: mutsuki_bot_protocol::BotAccountRef {
                account_id: "bot".into(),
                platform: mutsuki_bot_protocol::BotPlatform::QqBot,
            },
            kind: mutsuki_bot_protocol::BotEventKind::MessageCreated,
            time_ms: 1,
            target: BotTarget::Group {
                group_id: "g1".into(),
            },
            actor: None,
            message: None,
            raw: None,
            ext: BTreeMap::default(),
        };
        attach_bound_persona(store.as_ref(), &mut event).unwrap();
        assert_eq!(
            event.ext[BOT_EXT_AGENT_PROFILE_ID].as_str(),
            Some("qq-guide")
        );
        assert!(
            event.ext[BOT_EXT_PERSONA_PROMPT]
                .as_str()
                .is_some_and(|value| value.contains("你是向导"))
        );
    }
}
