use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{
    ArtifactType, LifecyclePolicy, PermissionGrant, PluginArtifact, PluginManifest, PluginProvides,
    RunnerDescriptor, RunnerPurity, ScalarValue,
};
use serde_json::json;

pub const PLUGIN_ID: &str = "mutsuki.qqbot";
pub const PLUGIN_VERSION: &str = "0.1.0";
pub const PLUGIN_API_VERSION: &str = "mutsuki-plugin-v1";

pub const RAW_GATEWAY_TASK_KIND: &str = "raw.input.qqbot.gateway";

pub const GATEWAY_NORMALIZER_RUNNER_ID: &str = "mutsuki.qqbot.gateway.normalize";
pub const EFFECT_RUNNER_ID: &str = "effect.qqbot.openapi";

pub const EFFECT_MESSAGE_SEND: &str = "effect.qqbot.message.send";
pub const EFFECT_MEDIA_UPLOAD: &str = "effect.qqbot.media.upload";
pub const EFFECT_MESSAGE_RECALL: &str = "effect.qqbot.message.recall";
pub const EFFECT_INTERACTION_ACK: &str = "effect.qqbot.interaction.ack";
pub const EFFECT_USER_SHARE_LINK: &str = "effect.qqbot.user.share_link";

pub const STREAM_GATEWAY: &str = "qqbot.gateway";
pub const SUBSCRIPTION_GATEWAY: &str = "qqbot.gateway.events";
pub const TIMER_GATEWAY_HEARTBEAT: &str = "qqbot.gateway.heartbeat";

pub fn gateway_normalizer_descriptor(plugin_generation: u64) -> RunnerDescriptor {
    RunnerDescriptor {
        runner_id: GATEWAY_NORMALIZER_RUNNER_ID.into(),
        plugin_id: PLUGIN_ID.into(),
        plugin_generation,
        accepted_task_kinds: vec![RAW_GATEWAY_TASK_KIND.into()],
        purity: RunnerPurity::Pure,
        input_schema: json!({
            "type": "object",
            "required": ["op"],
            "additionalProperties": true
        }),
        output_schema: json!({
            "events": ["qqbot.gateway.*", "qqbot.message.*", "qqbot.interaction", "qqbot.lifecycle", "qqbot.reaction"]
        }),
        metadata: metadata("QQBot Gateway dispatch normalizer"),
        contract_surfaces: vec![
            format!("runner:{GATEWAY_NORMALIZER_RUNNER_ID}"),
            format!("task_kind:{RAW_GATEWAY_TASK_KIND}"),
        ],
    }
}

pub fn openapi_effect_descriptor(plugin_generation: u64) -> RunnerDescriptor {
    RunnerDescriptor {
        runner_id: EFFECT_RUNNER_ID.into(),
        plugin_id: PLUGIN_ID.into(),
        plugin_generation,
        accepted_task_kinds: effect_task_kinds(),
        purity: RunnerPurity::Effectful,
        input_schema: json!({
            "type": "object",
            "additionalProperties": true
        }),
        output_schema: json!({
            "events": ["qqbot.openapi.result"]
        }),
        metadata: metadata("QQBot OpenAPI effect runner"),
        contract_surfaces: vec![format!("runner:{EFFECT_RUNNER_ID}")],
    }
}

pub fn effect_task_kinds() -> Vec<String> {
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
            path: "plugins/mutsuki-qqbot".into(),
            sha256: "sha256:mutsuki.qqbot.local".into(),
        },
        provides: PluginProvides {
            runners,
            task_demands: Vec::new(),
            resource_schemas: vec!["qqbot.media.v1".into()],
            resource_providers: vec!["qqbot.media.provider".into()],
            effects: effect_task_kinds(),
            streams: vec![STREAM_GATEWAY.into()],
            subscriptions: vec![SUBSCRIPTION_GATEWAY.into()],
            timers: vec![TIMER_GATEWAY_HEARTBEAT.into()],
            state_schemas: Vec::new(),
        },
        requires: Vec::new(),
        permissions: PermissionGrant {
            effects: effect_task_kinds(),
            resources: vec!["qqbot.media.read".into()],
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
