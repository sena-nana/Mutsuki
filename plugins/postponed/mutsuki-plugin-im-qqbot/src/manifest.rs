use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{
    ArtifactType, LifecyclePolicy, PermissionGrant, PluginArtifact, PluginManifest, PluginProvides,
    RunnerDescriptor, RunnerPurity, ScalarValue,
};
use serde_json::json;

pub const PLUGIN_ID: &str = "mutsuki.experimental.im.qqbot";
pub const PLUGIN_VERSION: &str = "0.1.0";
pub const PLUGIN_API_VERSION: &str = "mutsuki-plugin-v1";

pub const RAW_GATEWAY_PROTOCOL_ID: &str = "mutsuki.im.qqbot.gateway.raw";

pub const GATEWAY_NORMALIZER_RUNNER_ID: &str = "mutsuki.im.qqbot.gateway.normalize";
pub const EFFECT_RUNNER_ID: &str = "mutsuki.im.qqbot.openapi";

pub const EFFECT_MESSAGE_SEND: &str = "mutsuki.im.qqbot.message.send";
pub const EFFECT_MEDIA_UPLOAD: &str = "mutsuki.im.qqbot.media.upload";
pub const EFFECT_MESSAGE_RECALL: &str = "mutsuki.im.qqbot.message.recall";
pub const EFFECT_INTERACTION_ACK: &str = "mutsuki.im.qqbot.interaction.ack";
pub const EFFECT_USER_SHARE_LINK: &str = "mutsuki.im.qqbot.user.share_link";

pub const STREAM_GATEWAY: &str = "mutsuki.im.qqbot.gateway";
pub const SUBSCRIPTION_GATEWAY: &str = "mutsuki.im.qqbot.gateway.events";
pub const TIMER_GATEWAY_HEARTBEAT: &str = "mutsuki.im.qqbot.gateway.heartbeat";

pub fn gateway_normalizer_descriptor(plugin_generation: u64) -> RunnerDescriptor {
    RunnerDescriptor {
        runner_id: GATEWAY_NORMALIZER_RUNNER_ID.into(),
        plugin_id: PLUGIN_ID.into(),
        plugin_generation,
        accepted_protocol_ids: vec![RAW_GATEWAY_PROTOCOL_ID.into()],
        purity: RunnerPurity::Pure,
        input_schema: json!({
            "type": "object",
            "required": ["op"],
            "additionalProperties": true
        }),
        output_schema: json!({
            "events": ["mutsuki.im.qqbot.gateway.*", "mutsuki.im.qqbot.message.*", "mutsuki.im.qqbot.interaction", "mutsuki.im.qqbot.lifecycle", "mutsuki.im.qqbot.reaction"]
        }),
        metadata: metadata("QQBot Gateway dispatch normalizer"),
        contract_surfaces: vec![
            format!("runner:{GATEWAY_NORMALIZER_RUNNER_ID}"),
            format!("task_protocol:{RAW_GATEWAY_PROTOCOL_ID}"),
        ],
    }
}

pub fn openapi_effect_descriptor(plugin_generation: u64) -> RunnerDescriptor {
    RunnerDescriptor {
        runner_id: EFFECT_RUNNER_ID.into(),
        plugin_id: PLUGIN_ID.into(),
        plugin_generation,
        accepted_protocol_ids: effect_protocol_ids(),
        purity: RunnerPurity::Effectful,
        input_schema: json!({
            "type": "object",
            "additionalProperties": true
        }),
        output_schema: json!({
            "events": ["mutsuki.im.qqbot.openapi.result"]
        }),
        metadata: metadata("QQBot OpenAPI effect runner"),
        contract_surfaces: vec![format!("runner:{EFFECT_RUNNER_ID}")],
    }
}

pub fn effect_protocol_ids() -> Vec<String> {
    vec![
        EFFECT_MESSAGE_SEND.into(),
        EFFECT_MEDIA_UPLOAD.into(),
        EFFECT_MESSAGE_RECALL.into(),
        EFFECT_INTERACTION_ACK.into(),
        EFFECT_USER_SHARE_LINK.into(),
    ]
}

pub fn qqbot_manifest() -> PluginManifest {
    let runners = vec![
        gateway_normalizer_descriptor(1),
        openapi_effect_descriptor(1),
    ];
    PluginManifest {
        plugin_id: PLUGIN_ID.into(),
        version: PLUGIN_VERSION.into(),
        api_version: PLUGIN_API_VERSION.into(),
        artifact: PluginArtifact {
            artifact_type: ArtifactType::Native,
            path: "plugins/postponed/mutsuki-plugin-im-qqbot".into(),
            sha256: "sha256:mutsuki.experimental.im.qqbot.local".into(),
        },
        provides: PluginProvides {
            runners,
            protocols: Vec::new(),
            handler_bindings: Vec::new(),
            resource_schemas: vec!["mutsuki.im.qqbot.media.v1".into()],
            resource_providers: vec!["mutsuki.im.qqbot.media.provider".into()],
            effects: effect_protocol_ids(),
            streams: vec![STREAM_GATEWAY.into()],
            subscriptions: vec![SUBSCRIPTION_GATEWAY.into()],
            timers: vec![TIMER_GATEWAY_HEARTBEAT.into()],
            state_schemas: Vec::new(),
        },
        requires: Vec::new(),
        permissions: PermissionGrant {
            effects: effect_protocol_ids(),
            resources: vec!["mutsuki.im.qqbot.media.read".into()],
        },
        lifecycle: LifecyclePolicy {
            reload_policy: "drain_and_swap".into(),
            unload_timeout_ms: 5000,
            supports_cancel: true,
            supports_dispose: true,
            supports_snapshot: false,
        },
        metadata: metadata("QQBot Gateway and OpenAPI adapter"),
    }
}

fn metadata(description: &str) -> BTreeMap<String, ScalarValue> {
    BTreeMap::from([
        (
            "description".into(),
            ScalarValue::String(description.into()),
        ),
        ("domain".into(), ScalarValue::String("qqbot".into())),
    ])
}
