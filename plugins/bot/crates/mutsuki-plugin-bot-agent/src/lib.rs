// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::field_reassign_with_default,
    clippy::if_same_then_else,
    clippy::large_futures,
    clippy::map_unwrap_or,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::redundant_closure_for_method_calls,
    clippy::too_many_lines,
    clippy::unused_async
)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mutsuki_agent_client::{AgentClient, AgentClientBackend};
use mutsuki_agent_contracts::{
    AGENT_MESSAGE_CONTEXT_INJECTIONS_META, AgentContentPart, AgentEvent, AgentEventPage,
    AgentMessage, AgentMessageContextInjection, AgentSession, AgentSessionCreateRequest,
    AgentWireError, CONTEXT_SOURCE_ICL, CONTEXT_SOURCE_IDENTIFIERS, CONTEXT_SOURCE_PERSONA,
    SessionSnapshotRef, SessionVersion,
};
use mutsuki_bot_conversation::{
    AgentEventClaim, ConversationError, ConversationService, qq_conversation_from_event,
    session_binding_key,
};
#[cfg(test)]
use mutsuki_bot_protocol::BotNodeWiring;
use mutsuki_bot_protocol::{
    AgentSessionBinding, BOT_AGENT_BIND_PROFILE_PROTOCOL_ID, BOT_AGENT_CANCEL_PROTOCOL_ID,
    BOT_AGENT_FORK_PROTOCOL_ID, BOT_AGENT_REGENERATE_PROTOCOL_ID, BOT_AGENT_RESET_PROTOCOL_ID,
    BOT_AGENT_STATUS_PROTOCOL_ID, BOT_AGENT_SUBMIT_PROTOCOL_ID, BOT_EXT_AGENT_PROFILE_ID,
    BOT_EXT_CONVERSATION_ICL, BOT_EXT_CONVERSATION_IDENTIFIERS, BOT_EXT_PERSONA_PROMPT,
    BOT_EXT_REPLY_SOURCE_MESSAGE_ID, BOT_EXT_REPLY_SOURCE_USER_ID, BOT_FLOW_BOT_EVENT_TYPE,
    BOT_FLOW_DELIVERY_REPLY_TYPE, BOT_MEDIA_SYNTHESIZE_PROTOCOL_ID,
    BOT_MEDIA_TRANSCRIBE_PROTOCOL_ID, BOT_REPLY_DELIVERY_PROTOCOL_ID, BotAgentBridgeRequest,
    BotCommandEvent, BotDeliveryContent, BotEvent, BotFlowContext, BotFlowEventEnvelope,
    BotFlowPayload, BotFlowTypeRef, BotMediaKind, BotMediaSynthesizeRequest,
    BotMediaSynthesizeResult, BotMediaTranscribeRequest, BotMediaTranscribeResult, BotMessage,
    BotNodeBinding, BotNodeCatalogFragment, BotNodeDescriptor, BotNodeInvocation, BotNodeOutput,
    BotNodePortDescriptor, BotNodePortDirection, BotNodeResult, BotNodeRole,
    BotReplyDeliveryCommand, BotReplyDeliveryPart, BotReplyDeliveryReceipt,
    BotReplyDeliveryRequest, BotSpeechReplyPolicy, BotStreamingStrategy, BotTarget,
    ConversationIclEntry, ConversationIdentifiers, DeliveryPolicy, MessageSegment,
    ResolvedConversationPolicy, format_icl_summary,
};
use mutsuki_runtime_contracts::{
    ExecutionClass, InvocationMode, PluginManifest, RunnerBatchCapability, RunnerConcurrency,
    RunnerControlCapability, RunnerMode, RunnerResult, RunnerSideEffect, Task, TaskOutcome,
    TimeoutGranularity,
};
use mutsuki_runtime_sdk::Runner;
use mutsuki_runtime_sdk::{
    AsyncRunnerContext, BoxedTaskAwaitRunner, PluginBuilder, ProtocolDescriptorBuilder,
    RunnerDescriptorBuilder, RuntimeClientRef, RuntimeFailure, RuntimeResult,
    TaskAwaitRunnerAdapter,
};
use sha2::{Digest, Sha256};
use thiserror::Error;

mod config;

pub use config::{
    BOT_AGENT_CONFIG_PROVIDER_ID, BOT_AGENT_CONFIG_SERVICE_ID, BOT_AGENT_DEFAULT_MAX_MESSAGE_BYTES,
    BOT_AGENT_MIN_MESSAGE_BYTES, BotAgentConfig, BotAgentConfigError, BotAgentConfigHandle,
    BotAgentConfigSnapshot, bot_agent_config_schema,
};

pub const BOT_AGENT_BRIDGE_PLUGIN_ID: &str = "mutsuki.plugin.bot.agent";
pub const BOT_AGENT_BRIDGE_RUNNER_ID: &str = "mutsuki.bot.agent.bridge";
pub const BOT_AGENT_NODE_SUBMIT: &str = "mutsuki.bot.agent.submit";
pub const BOT_AGENT_NODE_CANCEL: &str = "mutsuki.bot.agent.cancel";
pub const BOT_AGENT_NODE_RESET: &str = "mutsuki.bot.agent.reset";
pub const BOT_AGENT_NODE_FORK: &str = "mutsuki.bot.agent.fork";
pub const BOT_AGENT_NODE_STATUS: &str = "mutsuki.bot.agent.status";
pub const BOT_AGENT_NODE_REGENERATE: &str = "mutsuki.bot.agent.regenerate";
pub const BOT_AGENT_NODE_BIND_PROFILE: &str = "mutsuki.bot.agent.bind_profile";

mod bridge;
mod client;
mod errors;
mod manifest;
mod runner;
#[cfg(test)]
mod tests;

pub use bridge::*;
pub use client::*;
pub use errors::*;
pub use manifest::*;
pub use runner::*;
