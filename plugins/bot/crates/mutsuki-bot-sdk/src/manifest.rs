//! Manifest helpers every Bot registration goes through.

use mutsuki_runtime_contracts::{PluginManifest, ProtocolDescriptor};
use mutsuki_runtime_sdk::ProtocolDescriptorBuilder;
use serde_json::json;

use crate::{BotManifestSurface, BotSubmissionGate};

/// Validates `manifest` against the outbound surface its tier is entitled to.
///
/// Rule 13 holds only if every registration passes through this check, so call sites name the
/// tier explicitly and widening a plugin's surface is a visible edit.
pub fn checked(
    manifest: PluginManifest,
    surface: BotManifestSurface,
) -> Result<PluginManifest, String> {
    BotSubmissionGate::ensure_manifest_surface(&manifest, surface).map_err(|failure| {
        let error = failure.error();
        format!("{}: {}", error.code, error.route)
    })?;
    Ok(manifest)
}

/// Merges two partial manifests of the same plugin into one registration.
pub fn merge(mut left: PluginManifest, right: PluginManifest) -> PluginManifest {
    debug_assert_eq!(left.plugin_id, right.plugin_id);
    left.provides
        .capabilities
        .extend(right.provides.capabilities);
    left.provides.runners.extend(right.provides.runners);
    left.provides.protocols.extend(right.provides.protocols);
    left.provides
        .protocol_classes
        .extend(right.provides.protocol_classes);
    left.provides
        .handler_bindings
        .extend(right.provides.handler_bindings);
    left.provides.extensions.extend(right.provides.extensions);
    left
}

/// Object protocol whose request and response must carry the listed fields and whose errors use
/// the standard `{code, source, route}` shape.
pub fn object_protocol_descriptor(
    protocol_id: &str,
    request_required: &[&str],
    response_required: &[&str],
) -> ProtocolDescriptor {
    ProtocolDescriptorBuilder::new(protocol_id)
        .input_schema(json!({"type": "object", "required": request_required}))
        .output_schema(json!({"type": "object", "required": response_required}))
        .error_schema(json!({
            "type": "object",
            "required": ["code", "source", "route"]
        }))
        .build()
}
