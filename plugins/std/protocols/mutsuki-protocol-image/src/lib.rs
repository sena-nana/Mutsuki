use mutsuki_runtime_contracts::ResourceRef;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const VERSION: &str = "0.1.0";
pub const ABI_CODEC: &str = "serde-json";
pub const RENDER: &str = "mutsuki.image.render";
pub const PROTOCOL_IDS: &[&str] = &[RENDER];
pub const PNG_SCHEMA: &str = "mutsuki.image.raster.png.v1";

pub const MAX_CANVAS_EDGE: u32 = 4096;
pub const MAX_SCENE_NODES: usize = 256;
pub const MAX_SCENE_DEPTH: usize = 8;
pub const MAX_BLUR_SIGMA: f32 = 64.0;
pub const MAX_TEXT_BYTES: usize = 16 * 1024;
pub const MAX_TEXT_LINES: u32 = 32;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgba {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub alpha: u8,
}

impl Rgba {
    pub const TRANSPARENT: Self = Self {
        red: 0,
        green: 0,
        blue: 0,
        alpha: 0,
    };
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Fill {
    Solid {
        color: Rgba,
    },
    LinearGradient {
        start: Point,
        end: Point,
        stops: Vec<GradientStop>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GradientStop {
    pub offset: f32,
    pub color: Rgba,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SceneEffect {
    GaussianBlur {
        sigma_x: f32,
        sigma_y: f32,
    },
    DropShadow {
        offset_x: f32,
        offset_y: f32,
        sigma_x: f32,
        sigma_y: f32,
        color: Rgba,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFit {
    Cover,
    Contain,
    Stretch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    Start,
    Center,
    End,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SceneNode {
    Group {
        #[serde(default)]
        clip: Option<SceneRect>,
        #[serde(default = "full_opacity")]
        opacity: f32,
        #[serde(default)]
        effects: Vec<SceneEffect>,
        children: Vec<SceneNode>,
    },
    Rect {
        bounds: SceneRect,
        #[serde(default)]
        corner_radius: f32,
        fill: Fill,
        #[serde(default = "full_opacity")]
        opacity: f32,
        #[serde(default)]
        effects: Vec<SceneEffect>,
    },
    Image {
        bounds: SceneRect,
        source: Box<ResourceRef>,
        fit: ImageFit,
        #[serde(default)]
        corner_radius: f32,
        #[serde(default = "full_opacity")]
        opacity: f32,
        #[serde(default)]
        effects: Vec<SceneEffect>,
    },
    Text {
        bounds: SceneRect,
        text: String,
        font_families: Vec<String>,
        font_size: f32,
        font_weight: u16,
        line_height: f32,
        align: TextAlign,
        max_lines: u32,
        ellipsis: bool,
        color: Rgba,
        #[serde(default = "full_opacity")]
        opacity: f32,
        #[serde(default)]
        effects: Vec<SceneEffect>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageScene {
    pub width: u32,
    pub height: u32,
    pub background: Rgba,
    pub nodes: Vec<SceneNode>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageRenderRequest {
    pub scene: ImageScene,
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

/// Validates the protocol-wide resource and layout safety limits.
///
/// # Errors
///
/// Returns an [`ImageRenderError`] identifying the first invalid canvas,
/// node, effect, image reference, or text constraint.
pub fn validate_scene(scene: &ImageScene) -> Result<(), ImageRenderError> {
    if scene.width == 0
        || scene.height == 0
        || scene.width > MAX_CANVAS_EDGE
        || scene.height > MAX_CANVAS_EDGE
    {
        return invalid(
            "scene.canvas",
            format!("canvas must be between 1x1 and {MAX_CANVAS_EDGE}x{MAX_CANVAS_EDGE}"),
        );
    }
    let mut node_count = 0;
    validate_nodes(&scene.nodes, 1, &mut node_count)
}

fn validate_nodes(
    nodes: &[SceneNode],
    depth: usize,
    node_count: &mut usize,
) -> Result<(), ImageRenderError> {
    if depth > MAX_SCENE_DEPTH {
        return invalid(
            "scene.depth",
            format!("scene depth exceeds {MAX_SCENE_DEPTH}"),
        );
    }
    for node in nodes {
        *node_count += 1;
        if *node_count > MAX_SCENE_NODES {
            return invalid(
                "scene.nodes",
                format!("scene node count exceeds {MAX_SCENE_NODES}"),
            );
        }
        validate_node(node, depth, node_count)?;
    }
    Ok(())
}

fn validate_node(
    node: &SceneNode,
    depth: usize,
    node_count: &mut usize,
) -> Result<(), ImageRenderError> {
    match node {
        SceneNode::Group {
            clip,
            opacity,
            effects,
            children,
        } => {
            validate_opacity(*opacity)?;
            if let Some(clip) = clip {
                validate_rect(*clip)?;
            }
            validate_effects(effects)?;
            validate_nodes(children, depth + 1, node_count)
        }
        SceneNode::Rect {
            bounds,
            corner_radius,
            fill,
            opacity,
            effects,
        } => {
            validate_rect(*bounds)?;
            validate_radius(*corner_radius)?;
            validate_fill(fill)?;
            validate_opacity(*opacity)?;
            validate_effects(effects)
        }
        SceneNode::Image {
            bounds,
            source,
            corner_radius,
            opacity,
            effects,
            ..
        } => {
            validate_rect(*bounds)?;
            validate_radius(*corner_radius)?;
            validate_opacity(*opacity)?;
            validate_effects(effects)?;
            if source.ref_id.trim().is_empty() {
                return invalid("scene.image.resource", "image resource ref_id is empty");
            }
            Ok(())
        }
        SceneNode::Text {
            bounds,
            text,
            font_families,
            font_size,
            font_weight,
            line_height,
            max_lines,
            opacity,
            effects,
            ..
        } => validate_text(
            *bounds,
            text,
            font_families,
            *font_size,
            *font_weight,
            *line_height,
            *max_lines,
            *opacity,
            effects,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_text(
    bounds: SceneRect,
    text: &str,
    font_families: &[String],
    font_size: f32,
    font_weight: u16,
    line_height: f32,
    max_lines: u32,
    opacity: f32,
    effects: &[SceneEffect],
) -> Result<(), ImageRenderError> {
    validate_rect(bounds)?;
    if text.len() > MAX_TEXT_BYTES {
        return invalid(
            "scene.text.length",
            format!("text exceeds {MAX_TEXT_BYTES} UTF-8 bytes"),
        );
    }
    if font_families.is_empty() || font_families.iter().any(|family| family.trim().is_empty()) {
        return invalid("scene.text.font", "font_families must not be empty");
    }
    if !font_size.is_finite() || font_size <= 0.0 {
        return invalid("scene.text.size", "font_size must be finite and positive");
    }
    if !(1..=1000).contains(&font_weight) {
        return invalid(
            "scene.text.weight",
            "font_weight must be between 1 and 1000",
        );
    }
    if !line_height.is_finite() || line_height <= 0.0 {
        return invalid(
            "scene.text.line_height",
            "line_height must be finite and positive",
        );
    }
    if max_lines == 0 || max_lines > MAX_TEXT_LINES {
        return invalid(
            "scene.text.lines",
            format!("max_lines must be between 1 and {MAX_TEXT_LINES}"),
        );
    }
    validate_opacity(opacity)?;
    validate_effects(effects)
}

fn validate_rect(rect: SceneRect) -> Result<(), ImageRenderError> {
    if [rect.x, rect.y, rect.width, rect.height]
        .into_iter()
        .any(|value| !value.is_finite())
        || rect.width <= 0.0
        || rect.height <= 0.0
    {
        return invalid(
            "scene.bounds",
            "node bounds must be finite with positive width and height",
        );
    }
    Ok(())
}

fn validate_radius(radius: f32) -> Result<(), ImageRenderError> {
    if !radius.is_finite() || radius < 0.0 {
        return invalid(
            "scene.corner_radius",
            "corner_radius must be finite and non-negative",
        );
    }
    Ok(())
}

fn validate_opacity(opacity: f32) -> Result<(), ImageRenderError> {
    if !opacity.is_finite() || !(0.0..=1.0).contains(&opacity) {
        return invalid("scene.opacity", "opacity must be between 0 and 1");
    }
    Ok(())
}

fn validate_fill(fill: &Fill) -> Result<(), ImageRenderError> {
    if let Fill::LinearGradient { start, end, stops } = fill {
        if [start.x, start.y, end.x, end.y]
            .into_iter()
            .any(|value| !value.is_finite())
            || (start.x - end.x).hypot(start.y - end.y) <= f32::EPSILON
        {
            return invalid(
                "scene.gradient.vector",
                "gradient vector must be finite and non-zero",
            );
        }
        if stops.len() < 2
            || stops.len() > 32
            || stops
                .iter()
                .any(|stop| !stop.offset.is_finite() || !(0.0..=1.0).contains(&stop.offset))
            || stops.windows(2).any(|pair| pair[0].offset > pair[1].offset)
        {
            return invalid(
                "scene.gradient.stops",
                "gradient requires 2 to 32 ordered stops between 0 and 1",
            );
        }
    }
    Ok(())
}

fn validate_effects(effects: &[SceneEffect]) -> Result<(), ImageRenderError> {
    for effect in effects {
        if let SceneEffect::DropShadow {
            offset_x, offset_y, ..
        } = effect
            && (!offset_x.is_finite() || !offset_y.is_finite())
        {
            return invalid("scene.effect.shadow", "drop shadow offsets must be finite");
        }
        let (sigma_x, sigma_y) = match effect {
            SceneEffect::GaussianBlur { sigma_x, sigma_y }
            | SceneEffect::DropShadow {
                sigma_x, sigma_y, ..
            } => (*sigma_x, *sigma_y),
        };
        if !sigma_x.is_finite()
            || !sigma_y.is_finite()
            || sigma_x < 0.0
            || sigma_y < 0.0
            || sigma_x > MAX_BLUR_SIGMA
            || sigma_y > MAX_BLUR_SIGMA
        {
            return invalid(
                "scene.effect.blur",
                format!("blur sigma must be between 0 and {MAX_BLUR_SIGMA}"),
            );
        }
    }
    Ok(())
}

fn invalid<T>(code: &str, detail: impl Into<String>) -> Result<T, ImageRenderError> {
    Err(ImageRenderError {
        code: code.into(),
        detail: detail.into(),
    })
}

const fn full_opacity() -> f32 {
    1.0
}

#[must_use]
pub fn input_schema(protocol_id: &str) -> Option<Value> {
    (protocol_id == RENDER).then(|| {
        json!({
            "type": "object",
            "required": ["scene"],
            "properties": {"scene": {"type": "object"}}
        })
    })
}

#[must_use]
pub fn output_schema(protocol_id: &str) -> Option<Value> {
    (protocol_id == RENDER).then(|| {
        json!({
            "type": "object",
            "required": ["resource", "width", "height", "byte_len"]
        })
    })
}

#[must_use]
pub fn error_schema(protocol_id: &str) -> Option<Value> {
    (protocol_id == RENDER).then(|| {
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

    fn sample_scene() -> ImageScene {
        ImageScene {
            width: 1200,
            height: 630,
            background: Rgba::TRANSPARENT,
            nodes: vec![SceneNode::Group {
                clip: Some(SceneRect {
                    x: 0.0,
                    y: 0.0,
                    width: 1200.0,
                    height: 630.0,
                }),
                opacity: 0.9,
                effects: vec![SceneEffect::DropShadow {
                    offset_x: 0.0,
                    offset_y: 8.0,
                    sigma_x: 12.0,
                    sigma_y: 12.0,
                    color: Rgba {
                        red: 0,
                        green: 0,
                        blue: 0,
                        alpha: 128,
                    },
                }],
                children: vec![
                    SceneNode::Rect {
                        bounds: SceneRect {
                            x: 10.0,
                            y: 10.0,
                            width: 200.0,
                            height: 100.0,
                        },
                        corner_radius: 16.0,
                        fill: Fill::LinearGradient {
                            start: Point { x: 0.0, y: 0.0 },
                            end: Point { x: 1.0, y: 1.0 },
                            stops: sample_stops(),
                        },
                        opacity: 1.0,
                        effects: vec![SceneEffect::GaussianBlur {
                            sigma_x: 2.0,
                            sigma_y: 2.0,
                        }],
                    },
                    SceneNode::Image {
                        bounds: SceneRect {
                            x: 0.0,
                            y: 0.0,
                            width: 100.0,
                            height: 100.0,
                        },
                        source: Box::new(resource()),
                        fit: ImageFit::Cover,
                        corner_radius: 8.0,
                        opacity: 1.0,
                        effects: Vec::new(),
                    },
                    SceneNode::Text {
                        bounds: SceneRect {
                            x: 20.0,
                            y: 20.0,
                            width: 500.0,
                            height: 120.0,
                        },
                        text: "米画师".into(),
                        font_families: vec!["Noto Sans SC".into()],
                        font_size: 48.0,
                        font_weight: 700,
                        line_height: 1.2,
                        align: TextAlign::Start,
                        max_lines: 2,
                        ellipsis: true,
                        color: Rgba {
                            red: 255,
                            green: 255,
                            blue: 255,
                            alpha: 255,
                        },
                        opacity: 1.0,
                        effects: Vec::new(),
                    },
                ],
            }],
        }
    }

    fn sample_stops() -> Vec<GradientStop> {
        vec![
            GradientStop {
                offset: 0.0,
                color: Rgba {
                    red: 255,
                    green: 0,
                    blue: 0,
                    alpha: 255,
                },
            },
            GradientStop {
                offset: 1.0,
                color: Rgba {
                    red: 0,
                    green: 0,
                    blue: 255,
                    alpha: 255,
                },
            },
        ]
    }

    #[test]
    fn every_scene_variant_round_trips() {
        let scene = sample_scene();
        let value = serde_json::to_value(&scene).unwrap();
        assert_eq!(serde_json::from_value::<ImageScene>(value).unwrap(), scene);
        assert!(validate_scene(&scene).is_ok());
    }

    #[test]
    fn limits_reject_invalid_canvas_depth_nodes_blur_and_text() {
        let mut scene = sample_scene();
        scene.width = MAX_CANVAS_EDGE + 1;
        assert_eq!(validate_scene(&scene).unwrap_err().code, "scene.canvas");

        scene = sample_scene();
        scene.nodes = (0..=MAX_SCENE_NODES)
            .map(|_| scene.nodes[0].clone())
            .collect();
        assert_eq!(validate_scene(&scene).unwrap_err().code, "scene.nodes");

        scene = sample_scene();
        scene.nodes = vec![nested_group(MAX_SCENE_DEPTH + 1)];
        assert_eq!(validate_scene(&scene).unwrap_err().code, "scene.depth");

        scene = sample_scene();
        if let SceneNode::Group { effects, .. } = &mut scene.nodes[0] {
            effects[0] = SceneEffect::GaussianBlur {
                sigma_x: MAX_BLUR_SIGMA + 1.0,
                sigma_y: 1.0,
            };
        }
        assert_eq!(
            validate_scene(&scene).unwrap_err().code,
            "scene.effect.blur"
        );

        scene = sample_scene();
        if let SceneNode::Group { children, .. } = &mut scene.nodes[0]
            && let SceneNode::Text { text, .. } = &mut children[2]
        {
            *text = "x".repeat(MAX_TEXT_BYTES + 1);
        }
        assert_eq!(
            validate_scene(&scene).unwrap_err().code,
            "scene.text.length"
        );
    }

    fn nested_group(depth: usize) -> SceneNode {
        if depth == 1 {
            return SceneNode::Group {
                clip: None,
                opacity: 1.0,
                effects: Vec::new(),
                children: Vec::new(),
            };
        }
        SceneNode::Group {
            clip: None,
            opacity: 1.0,
            effects: Vec::new(),
            children: vec![nested_group(depth - 1)],
        }
    }
}
