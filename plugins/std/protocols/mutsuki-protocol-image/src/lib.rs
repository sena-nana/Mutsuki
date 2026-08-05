//! Image render protocols: compose / card / QR → PNG ResourceRef.

use mutsuki_runtime_contracts::ResourceRef;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const VERSION: &str = "0.1.0";
pub const ABI_CODEC: &str = "serde-json";
pub const COMPOSE: &str = "mutsuki.image.compose";
pub const CARD_RENDER: &str = "mutsuki.image.card.render";
pub const QR_RENDER: &str = "mutsuki.image.qr.render";
pub const PROTOCOL_IDS: &[&str] = &[COMPOSE, CARD_RENDER, QR_RENDER];
pub const PNG_SCHEMA: &str = "mutsuki.image.raster.png.v1";

pub const MAX_CANVAS_EDGE: u32 = 4096;
pub const MAX_HTML_BYTES: usize = 256 * 1024;
pub const MAX_STYLESHEET_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgba {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub alpha: u8,
}

/// HTML/CSS template payload for [`COMPOSE`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposeRenderRequest {
    pub html: String,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub stylesheet: Option<String>,
}

/// Content and brand styling for the standard 1200x630 social link card.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CardRenderRequest {
    pub brand: String,
    pub title: String,
    pub description: String,
    pub url: String,
    #[serde(default)]
    pub cover: Option<ResourceRef>,
    pub fallback_gradient: CardGradient,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardGradient {
    pub start: Rgba,
    pub end: Rgba,
}

/// Text content for a square QR raster.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QrRenderRequest {
    pub content: String,
    pub min_dimensions: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageRenderResponse {
    pub resource: ResourceRef,
    pub width: u32,
    pub height: u32,
    pub byte_len: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageRenderError {
    pub code: String,
    pub detail: String,
}

/// Validates a [`ComposeRenderRequest`] canvas and markup size limits.
///
/// # Errors
///
/// Returns an [`ImageRenderError`] when the canvas is out of range or the
/// HTML/stylesheet payload exceeds the protocol byte caps.
pub fn validate_compose(request: &ComposeRenderRequest) -> Result<(), ImageRenderError> {
    if request.width == 0
        || request.height == 0
        || request.width > MAX_CANVAS_EDGE
        || request.height > MAX_CANVAS_EDGE
    {
        return Err(ImageRenderError {
            code: "compose.canvas".into(),
            detail: format!("canvas must be between 1x1 and {MAX_CANVAS_EDGE}x{MAX_CANVAS_EDGE}"),
        });
    }
    if request.html.trim().is_empty() {
        return Err(ImageRenderError {
            code: "compose.html".into(),
            detail: "html must not be empty".into(),
        });
    }
    if request.html.len() > MAX_HTML_BYTES {
        return Err(ImageRenderError {
            code: "compose.html.length".into(),
            detail: format!("html exceeds {MAX_HTML_BYTES} UTF-8 bytes"),
        });
    }
    if request
        .stylesheet
        .as_ref()
        .is_some_and(|css| css.len() > MAX_STYLESHEET_BYTES)
    {
        return Err(ImageRenderError {
            code: "compose.stylesheet.length".into(),
            detail: format!("stylesheet exceeds {MAX_STYLESHEET_BYTES} UTF-8 bytes"),
        });
    }
    Ok(())
}

#[must_use]
pub fn input_schema(protocol_id: &str) -> Option<Value> {
    match protocol_id {
        COMPOSE => Some(json!({
            "type": "object",
            "required": ["html", "width", "height"],
            "properties": {
                "html": {"type": "string", "maxLength": MAX_HTML_BYTES},
                "width": {"type": "integer", "minimum": 1, "maximum": MAX_CANVAS_EDGE},
                "height": {"type": "integer", "minimum": 1, "maximum": MAX_CANVAS_EDGE},
                "stylesheet": {"type": ["string", "null"], "maxLength": MAX_STYLESHEET_BYTES}
            }
        })),
        CARD_RENDER => Some(json!({
            "type": "object",
            "required": ["brand", "title", "description", "url", "fallback_gradient"],
            "properties": {
                "brand": {"type": "string"},
                "title": {"type": "string"},
                "description": {"type": "string"},
                "url": {"type": "string"},
                "cover": {"type": ["object", "null"]},
                "fallback_gradient": {"type": "object"}
            }
        })),
        QR_RENDER => Some(json!({
            "type": "object",
            "required": ["content", "min_dimensions"],
            "properties": {
                "content": {"type": "string"},
                "min_dimensions": {"type": "integer", "minimum": 1, "maximum": 4096}
            }
        })),
        _ => None,
    }
}

#[must_use]
pub fn output_schema(protocol_id: &str) -> Option<Value> {
    PROTOCOL_IDS.contains(&protocol_id).then(|| {
        json!({
            "type": "object",
            "required": ["resource", "width", "height", "byte_len"]
        })
    })
}

#[must_use]
pub fn error_schema(protocol_id: &str) -> Option<Value> {
    PROTOCOL_IDS.contains(&protocol_id).then(|| {
        json!({
            "type": "object",
            "required": ["code", "detail"]
        })
    })
}

#[cfg(test)]
mod tests {
    use mutsuki_runtime_contracts::{
        ResourceAccess, ResourceId, ResourceLifetime, ResourceSealState, ResourceSemantic,
    };

    use super::*;

    fn resource() -> ResourceRef {
        ResourceRef {
            ref_id: "image-1".into(),
            resource_id: ResourceId {
                kind_id: "blob".into(),
                slot_id: "image-1".into(),
                generation: 1,
                version: 1,
            },
            semantic: ResourceSemantic::FrozenValue,
            provider_id: "memory".into(),
            resource_kind: "blob".into(),
            schema: "image/jpeg".into(),
            version: 1,
            generation: 1,
            access: ResourceAccess::ProviderRpc {
                provider_id: "memory".into(),
                method: "memory".into(),
            },
            size_hint: Some(32),
            content_hash: None,
            lifetime: ResourceLifetime::Persistent,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        }
    }

    #[test]
    fn contracts_round_trip_and_publish_schemas() {
        let compose = ComposeRenderRequest {
            html: r#"<div style="display:flex">卡片</div>"#.into(),
            width: 1200,
            height: 630,
            stylesheet: Some(".card { display: grid; }".into()),
        };
        assert_eq!(
            serde_json::from_value::<ComposeRenderRequest>(serde_json::to_value(&compose).unwrap())
                .unwrap(),
            compose
        );
        assert!(validate_compose(&compose).is_ok());

        let card = CardRenderRequest {
            brand: "米画师".into(),
            title: "Painter".into(),
            description: "Window".into(),
            url: "https://www.mihuashi.com/profiles/1".into(),
            cover: Some(resource()),
            fallback_gradient: CardGradient {
                start: Rgba {
                    red: 240,
                    green: 91,
                    blue: 122,
                    alpha: 255,
                },
                end: Rgba {
                    red: 91,
                    green: 72,
                    blue: 176,
                    alpha: 255,
                },
            },
        };
        assert_eq!(
            serde_json::from_value::<CardRenderRequest>(serde_json::to_value(&card).unwrap())
                .unwrap(),
            card
        );

        let qr = QrRenderRequest {
            content: "https://www.bilibili.com".into(),
            min_dimensions: 256,
        };
        assert_eq!(
            serde_json::from_value::<QrRenderRequest>(serde_json::to_value(&qr).unwrap()).unwrap(),
            qr
        );

        for protocol_id in PROTOCOL_IDS {
            assert!(input_schema(protocol_id).is_some());
            assert!(output_schema(protocol_id).is_some());
            assert!(error_schema(protocol_id).is_some());
        }
    }

    #[test]
    fn compose_limits_reject_empty_and_oversized_html() {
        assert_eq!(
            validate_compose(&ComposeRenderRequest {
                html: "   ".into(),
                width: 100,
                height: 100,
                stylesheet: None,
            })
            .unwrap_err()
            .code,
            "compose.html"
        );
        assert_eq!(
            validate_compose(&ComposeRenderRequest {
                html: "x".repeat(MAX_HTML_BYTES + 1),
                width: 100,
                height: 100,
                stylesheet: None,
            })
            .unwrap_err()
            .code,
            "compose.html.length"
        );
    }
}
