use std::path::PathBuf;
use std::sync::{Arc, Once};

use mutsuki_protocol_image::{
    Fill, ImageFit, ImageRenderRequest, ImageRenderResponse, PNG_SCHEMA, Point, RENDER, Rgba,
    SceneEffect, SceneNode, SceneRect, TextAlign, validate_scene,
};
use mutsuki_runtime_contracts::{
    CompletionBatch, ExecutionClass, ProtocolClass, ReadPlan, RunnerBatchCapability, RunnerContext,
    RunnerDescriptor, RunnerMode, RunnerPurity, RunnerResult, RunnerSideEffect, RuntimeError,
    ScalarValue, Task, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RuntimeResult};
use mutsuki_runtime_sdk::{
    PluginBuilder, ProtocolDescriptorBuilder, ResourceRegistryGateway, RunnerDescriptorBuilder,
    map_work_batch_entries,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use skia_safe::canvas::{SaveLayerRec, SrcRectConstraint};
use skia_safe::font_style::{Slant, Weight, Width};
use skia_safe::gradient::{Colors, Gradient, Interpolation, shaders};
use skia_safe::textlayout::{
    FontCollection, ParagraphBuilder, ParagraphStyle, TextStyle, TypefaceFontProvider,
};
use skia_safe::{
    Canvas, Color, Color4f, Data, EncodedImageFormat, FontMgr, FontStyle, Image, ImageFilter,
    Paint, RRect, Rect, Surface, TileMode, Typeface, image_filters, surfaces,
};

pub const PLUGIN_ID: &str = "mutsuki.std.image.render.skia";
pub const RUNNER_ID: &str = "mutsuki.std.image.render.skia.runner";
pub const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_TOTAL_INPUT_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;

static INIT_ICU: Once = Once::new();

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkiaRenderConfig {
    pub output_provider_id: String,
    pub font_files: Vec<PathBuf>,
}

impl SkiaRenderConfig {
    /// Validates provider selection and explicit deployment font paths.
    ///
    /// # Errors
    ///
    /// Returns an error when the provider is empty or any font path is
    /// missing, relative, or not a file.
    pub fn validate(&self) -> Result<(), String> {
        if self.output_provider_id.trim().is_empty() {
            return Err("output_provider_id is required".into());
        }
        if self.font_files.is_empty() {
            return Err("font_files must contain at least one explicit font file".into());
        }
        for path in &self.font_files {
            if !path.is_absolute() || !path.is_file() {
                return Err(format!(
                    "font file is missing or not an absolute file: {}",
                    path.display()
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
struct FontAsset {
    family: String,
    typeface: Typeface,
}

pub struct SkiaRenderRunner {
    descriptor: RunnerDescriptor,
    output_provider_id: String,
    fonts: Vec<FontAsset>,
    resources: Arc<dyn ResourceRegistryGateway>,
}

impl SkiaRenderRunner {
    /// Loads and validates all configured font assets before serving work.
    ///
    /// # Errors
    ///
    /// Returns an error when configuration validation, font I/O, or Skia
    /// typeface decoding fails.
    pub fn launch(
        config: SkiaRenderConfig,
        resources: Arc<dyn ResourceRegistryGateway>,
    ) -> Result<Self, String> {
        config.validate()?;
        INIT_ICU.call_once(skia_safe::icu::init);
        let manager = FontMgr::new();
        let mut fonts = Vec::with_capacity(config.font_files.len());
        for path in &config.font_files {
            let bytes = std::fs::read(path)
                .map_err(|error| format!("failed to read font {}: {error}", path.display()))?;
            let typeface = manager
                .new_from_data(&bytes, None)
                .ok_or_else(|| format!("invalid or unsupported font file: {}", path.display()))?;
            let family = typeface.family_name();
            if family.trim().is_empty() {
                return Err(format!("font has no family name: {}", path.display()));
            }
            fonts.push(FontAsset { family, typeface });
        }
        Ok(Self {
            descriptor: runner_descriptor(),
            output_provider_id: config.output_provider_id,
            fonts,
            resources,
        })
    }

    fn run_task(&self, task: &Task) -> Result<RunnerResult, RuntimeError> {
        let request: ImageRenderRequest = serde_json::from_value(task.payload.to_value())
            .map_err(|error| render_error(task, "request.invalid", error.to_string()))?;
        validate_scene(&request.scene)
            .map_err(|error| render_error(task, error.code, error.detail))?;
        let bytes = self.render(task, &request)?;
        if bytes.len() > MAX_OUTPUT_BYTES {
            return Err(render_error(
                task,
                "output.oversized",
                format!(
                    "PNG output is {} bytes; maximum is {MAX_OUTPUT_BYTES}",
                    bytes.len()
                ),
            ));
        }
        let byte_len = bytes.len();
        let resource = self
            .resources
            .create_blob_resource(&self.output_provider_id, PNG_SCHEMA, bytes)
            .map_err(|error| render_error(task, "output.provider", error.to_string()))?;
        let response = ImageRenderResponse {
            resource: resource.clone(),
            width: request.scene.width,
            height: request.scene.height,
            byte_len,
        };
        let mut result = RunnerResult::completed(task.task_id.clone());
        result.output = Some(
            serde_json::to_value(response)
                .map_err(|error| render_error(task, "output.encode", error.to_string()))?,
        );
        result.resources.push(resource);
        Ok(result)
    }

    fn render(&self, task: &Task, request: &ImageRenderRequest) -> Result<Vec<u8>, RuntimeError> {
        let width = i32::try_from(request.scene.width)
            .map_err(|error| render_error(task, "surface.invalid", error.to_string()))?;
        let height = i32::try_from(request.scene.height)
            .map_err(|error| render_error(task, "surface.invalid", error.to_string()))?;
        let mut surface = surfaces::raster_n32_premul((width, height)).ok_or_else(|| {
            render_error(
                task,
                "surface.create",
                "failed to create CPU raster surface",
            )
        })?;
        surface.canvas().clear(color4f(request.scene.background));
        let mut state = RenderState {
            task,
            resources: self.resources.as_ref(),
            fonts: &self.fonts,
            total_input_bytes: 0,
        };
        draw_nodes(surface.canvas(), &request.scene.nodes, &mut state)?;
        encode_png(task, &mut surface)
    }
}

impl Runner for SkiaRenderRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        if ctx.cancel_requested {
            return Ok(CompletionBatch::from_error(
                &batch,
                RuntimeError::new(
                    "task.cancelled",
                    PLUGIN_ID,
                    format!("image.render.cancelled.{}", ctx.invocation_id),
                ),
            ));
        }
        map_work_batch_entries(&batch, |task| self.run_task(task))
    }
}

struct RenderState<'a> {
    task: &'a Task,
    resources: &'a dyn ResourceRegistryGateway,
    fonts: &'a [FontAsset],
    total_input_bytes: usize,
}

fn draw_nodes(
    canvas: &Canvas,
    nodes: &[SceneNode],
    state: &mut RenderState<'_>,
) -> Result<(), RuntimeError> {
    for node in nodes {
        draw_node(canvas, node, state)?;
    }
    Ok(())
}

fn draw_node(
    canvas: &Canvas,
    node: &SceneNode,
    state: &mut RenderState<'_>,
) -> Result<(), RuntimeError> {
    let (opacity, effects) = match node {
        SceneNode::Group {
            opacity, effects, ..
        }
        | SceneNode::Rect {
            opacity, effects, ..
        }
        | SceneNode::Image {
            opacity, effects, ..
        }
        | SceneNode::Text {
            opacity, effects, ..
        } => (*opacity, effects.as_slice()),
    };
    canvas.save();
    let mut layer_paint = Paint::default();
    layer_paint.set_alpha_f(opacity);
    if let Some(filter) = effect_filter(effects) {
        layer_paint.set_image_filter(filter);
    }
    canvas.save_layer(&SaveLayerRec::default().paint(&layer_paint));
    match node {
        SceneNode::Group { clip, children, .. } => {
            if let Some(clip) = clip {
                canvas.clip_rect(to_rect(*clip), None, true);
            }
            draw_nodes(canvas, children, state)?;
        }
        SceneNode::Rect {
            bounds,
            corner_radius,
            fill,
            ..
        } => draw_rect(canvas, *bounds, *corner_radius, fill),
        SceneNode::Image {
            bounds,
            source,
            fit,
            corner_radius,
            ..
        } => draw_image(canvas, *bounds, source, *fit, *corner_radius, state)?,
        SceneNode::Text {
            bounds,
            text,
            font_families,
            font_size,
            font_weight,
            line_height,
            align,
            max_lines,
            ellipsis,
            color,
            ..
        } => draw_text(
            canvas,
            *bounds,
            text,
            font_families,
            *font_size,
            *font_weight,
            *line_height,
            *align,
            *max_lines,
            *ellipsis,
            *color,
            state,
        )?,
    }
    canvas.restore();
    canvas.restore();
    Ok(())
}

fn draw_rect(canvas: &Canvas, bounds: SceneRect, corner_radius: f32, fill: &Fill) {
    let mut paint = fill_paint(fill);
    paint.set_anti_alias(true);
    let rect = to_rect(bounds);
    if corner_radius == 0.0 {
        canvas.draw_rect(rect, &paint);
    } else {
        canvas.draw_rrect(
            RRect::new_rect_xy(rect, corner_radius, corner_radius),
            &paint,
        );
    }
}

fn fill_paint(fill: &Fill) -> Paint {
    let mut paint = Paint::default();
    match fill {
        Fill::Solid { color } => {
            paint.set_color4f(color4f(*color), None);
        }
        Fill::LinearGradient { start, end, stops } => {
            let colors: Vec<_> = stops.iter().map(|stop| color4f(stop.color)).collect();
            let positions: Vec<_> = stops.iter().map(|stop| stop.offset).collect();
            let colors = Colors::new(&colors, Some(&positions), TileMode::Clamp, None);
            let gradient = Gradient::new(colors, Interpolation::default());
            if let Some(shader) =
                shaders::linear_gradient((to_point(*start), to_point(*end)), &gradient, None)
            {
                paint.set_shader(shader);
            }
        }
    }
    paint
}

fn draw_image(
    canvas: &Canvas,
    bounds: SceneRect,
    resource: &mutsuki_runtime_contracts::ResourceRef,
    fit: ImageFit,
    corner_radius: f32,
    state: &mut RenderState<'_>,
) -> Result<(), RuntimeError> {
    if resource
        .size_hint
        .is_some_and(|size| size > MAX_IMAGE_BYTES as u64)
    {
        return Err(render_error(
            state.task,
            "image.oversized",
            format!(
                "image resource {} exceeds {MAX_IMAGE_BYTES} bytes",
                resource.ref_id
            ),
        ));
    }
    let bytes = state
        .resources
        .collect_read_plan(&ReadPlan {
            plan_id: format!(
                "image.render.read.{}.{}",
                state.task.task_id, resource.ref_id
            ),
            resource: resource.clone(),
            operation: "collect".into(),
            args: Value::Null,
        })
        .map_err(|error| render_error(state.task, "image.read", error.to_string()))?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(render_error(
            state.task,
            "image.oversized",
            format!(
                "image resource {} is {} bytes",
                resource.ref_id,
                bytes.len()
            ),
        ));
    }
    state.total_input_bytes = state
        .total_input_bytes
        .checked_add(bytes.len())
        .ok_or_else(|| render_error(state.task, "image.total", "input byte count overflow"))?;
    if state.total_input_bytes > MAX_TOTAL_INPUT_BYTES {
        return Err(render_error(
            state.task,
            "image.total",
            format!("input images exceed {MAX_TOTAL_INPUT_BYTES} bytes"),
        ));
    }
    let image = Image::from_encoded(Data::new_copy(&bytes)).ok_or_else(|| {
        render_error(
            state.task,
            "image.decode",
            format!("failed to decode image resource {}", resource.ref_id),
        )
    })?;
    let max_edge = i32::try_from(mutsuki_protocol_image::MAX_CANVAS_EDGE).unwrap_or(i32::MAX);
    if image.width() <= 0
        || image.height() <= 0
        || image.width() > max_edge
        || image.height() > max_edge
    {
        return Err(render_error(
            state.task,
            "image.dimensions",
            format!(
                "decoded image dimensions are {}x{}",
                image.width(),
                image.height()
            ),
        ));
    }
    let dst = to_rect(bounds);
    canvas.save();
    if corner_radius > 0.0 {
        canvas.clip_rrect(
            RRect::new_rect_xy(dst, corner_radius, corner_radius),
            None,
            true,
        );
    }
    let image_width = u16::try_from(image.width())
        .map(f32::from)
        .map_err(|error| render_error(state.task, "image.dimensions", error.to_string()))?;
    let image_height = u16::try_from(image.height())
        .map(f32::from)
        .map_err(|error| render_error(state.task, "image.dimensions", error.to_string()))?;
    let (src, target) = fitted_rects(image_width, image_height, dst, fit);
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    canvas.draw_image_rect(
        image,
        Some((&src, SrcRectConstraint::Strict)),
        target,
        &paint,
    );
    canvas.restore();
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_text(
    canvas: &Canvas,
    bounds: SceneRect,
    text: &str,
    font_families: &[String],
    font_size: f32,
    font_weight: u16,
    line_height: f32,
    align: TextAlign,
    max_lines: u32,
    ellipsis: bool,
    color: Rgba,
    state: &RenderState<'_>,
) -> Result<(), RuntimeError> {
    let matching: Vec<_> = state
        .fonts
        .iter()
        .filter(|asset| font_families.iter().any(|family| family == &asset.family))
        .map(|asset| &asset.typeface)
        .collect();
    if matching.is_empty() {
        return Err(render_error(
            state.task,
            "text.font_missing",
            format!("none of the requested font families are configured: {font_families:?}"),
        ));
    }
    if let Some(character) = text
        .chars()
        .filter(|character| !character.is_whitespace())
        .find(|character| {
            let codepoint = i32::try_from(u32::from(*character)).unwrap_or_default();
            matching
                .iter()
                .all(|typeface| typeface.unichar_to_glyph(codepoint) == 0)
        })
    {
        return Err(render_error(
            state.task,
            "text.glyph_missing",
            format!(
                "configured fonts do not contain U+{:04X}",
                u32::from(character)
            ),
        ));
    }
    let mut provider = TypefaceFontProvider::new();
    for asset in state.fonts {
        provider.register_typeface(asset.typeface.clone(), Some(asset.family.as_str()));
    }
    let mut collection = FontCollection::new();
    collection.set_asset_font_manager(Some(provider.into()));

    let mut paragraph_style = ParagraphStyle::new();
    paragraph_style.set_text_align(match align {
        TextAlign::Start => skia_safe::textlayout::TextAlign::Left,
        TextAlign::Center => skia_safe::textlayout::TextAlign::Center,
        TextAlign::End => skia_safe::textlayout::TextAlign::Right,
    });
    paragraph_style.set_max_lines(Some(max_lines as usize));
    if ellipsis {
        paragraph_style.set_ellipsis("…");
    }
    let mut text_style = TextStyle::new();
    text_style
        .set_color(color8(color))
        .set_font_families(font_families)
        .set_font_size(font_size)
        .set_font_style(FontStyle::new(
            Weight::from(i32::from(font_weight)),
            Width::NORMAL,
            Slant::Upright,
        ))
        .set_height(line_height)
        .set_height_override(true);
    paragraph_style.set_text_style(&text_style);
    let mut builder = ParagraphBuilder::new(&paragraph_style, collection);
    builder.push_style(&text_style);
    builder.add_text(text);
    let mut paragraph = builder.build();
    paragraph.layout(bounds.width);
    canvas.save();
    canvas.clip_rect(to_rect(bounds), None, true);
    paragraph.paint(canvas, (bounds.x, bounds.y));
    canvas.restore();
    Ok(())
}

fn fitted_rects(image_width: f32, image_height: f32, dst: Rect, fit: ImageFit) -> (Rect, Rect) {
    let full = Rect::from_xywh(0.0, 0.0, image_width, image_height);
    match fit {
        ImageFit::Stretch => (full, dst),
        ImageFit::Cover => {
            let scale = (dst.width() / image_width).max(dst.height() / image_height);
            let src_width = dst.width() / scale;
            let src_height = dst.height() / scale;
            (
                Rect::from_xywh(
                    (image_width - src_width) / 2.0,
                    (image_height - src_height) / 2.0,
                    src_width,
                    src_height,
                ),
                dst,
            )
        }
        ImageFit::Contain => {
            let scale = (dst.width() / image_width).min(dst.height() / image_height);
            let width = image_width * scale;
            let height = image_height * scale;
            (
                full,
                Rect::from_xywh(
                    dst.left + (dst.width() - width) / 2.0,
                    dst.top + (dst.height() - height) / 2.0,
                    width,
                    height,
                ),
            )
        }
    }
}

fn effect_filter(effects: &[SceneEffect]) -> Option<ImageFilter> {
    let mut input = None;
    for effect in effects {
        input = match effect {
            SceneEffect::GaussianBlur { sigma_x, sigma_y } => {
                image_filters::blur((*sigma_x, *sigma_y), TileMode::Decal, input, None)
            }
            SceneEffect::DropShadow {
                offset_x,
                offset_y,
                sigma_x,
                sigma_y,
                color,
            } => image_filters::drop_shadow(
                (*offset_x, *offset_y),
                (*sigma_x, *sigma_y),
                color4f(*color),
                None,
                input,
                None,
            ),
        };
    }
    input
}

fn encode_png(task: &Task, surface: &mut Surface) -> Result<Vec<u8>, RuntimeError> {
    surface
        .image_snapshot()
        .encode(None, EncodedImageFormat::PNG, 100)
        .map(|data| data.as_bytes().to_vec())
        .ok_or_else(|| render_error(task, "output.encode", "Skia PNG encoding failed"))
}

fn to_rect(rect: SceneRect) -> Rect {
    Rect::from_xywh(rect.x, rect.y, rect.width, rect.height)
}

fn to_point(point: Point) -> skia_safe::Point {
    skia_safe::Point::new(point.x, point.y)
}

fn color4f(color: Rgba) -> Color4f {
    Color4f::new(
        f32::from(color.red) / 255.0,
        f32::from(color.green) / 255.0,
        f32::from(color.blue) / 255.0,
        f32::from(color.alpha) / 255.0,
    )
}

fn color8(color: Rgba) -> Color {
    Color::from_argb(color.alpha, color.red, color.green, color.blue)
}

#[must_use]
pub fn manifest() -> mutsuki_runtime_contracts::PluginManifest {
    let mut manifest = PluginBuilder::new(PLUGIN_ID)
        .runner(Box::new(ManifestOnlyRunner {
            descriptor: runner_descriptor(),
        }))
        .protocol_handler(protocol_descriptor(), RUNNER_ID, "cpu")
        .build()
        .manifest;
    manifest
        .provides
        .protocol_classes
        .insert(RENDER.into(), ProtocolClass::Effect);
    manifest
}

fn runner_descriptor() -> RunnerDescriptor {
    RunnerDescriptorBuilder::new(RUNNER_ID, PLUGIN_ID)
        .accepted_protocol(RENDER)
        .purity(RunnerPurity::Effectful)
        .execution_class(ExecutionClass::Cpu)
        .batch_capability(RunnerBatchCapability {
            mode: RunnerMode::ScalarAdapter,
            side_effect: RunnerSideEffect::External,
            ..Default::default()
        })
        .metadata("standard_plugin", ScalarValue::String("image_skia".into()))
        .build()
}

fn protocol_descriptor() -> mutsuki_runtime_contracts::ProtocolDescriptor {
    ProtocolDescriptorBuilder::new(RENDER)
        .input_schema(mutsuki_protocol_image::input_schema(RENDER).unwrap())
        .output_schema(mutsuki_protocol_image::output_schema(RENDER).unwrap())
        .error_schema(mutsuki_protocol_image::error_schema(RENDER).unwrap())
        .build()
}

struct ManifestOnlyRunner {
    descriptor: RunnerDescriptor,
}

impl Runner for ManifestOnlyRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        Ok(CompletionBatch::from_error(
            &batch,
            RuntimeError::new(
                mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
                PLUGIN_ID,
                "manifest_only_runner",
            ),
        ))
    }
}

fn render_error(task: &Task, route: impl Into<String>, detail: impl Into<String>) -> RuntimeError {
    let mut error = RuntimeError::new(
        "image.render_failed",
        PLUGIN_ID,
        format!("image.render.{}.{}", route.into(), task.task_id),
    );
    error
        .evidence
        .insert("detail".into(), ScalarValue::String(detail.into()));
    error
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use image::{ColorType, ImageEncoder, codecs::png::PngEncoder};
    use mutsuki_plugin_resource_memory::MemoryResourceProvider;
    use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
    use mutsuki_runtime_contracts::{
        BatchEntry, BatchPayload, CommandPlan, DispatchLane, ExportPlan, OrderingRequirement,
        PlanReceipt, ResourceRef, SnapshotDescriptor, StreamPlan, WorkResourcePlan, WritePlan,
    };
    use mutsuki_runtime_core::RuntimeFailure;
    use mutsuki_runtime_sdk::{ResourcePlanGateway, ResourceProviderGateway};

    use super::*;

    struct TestGateway {
        provider: MemoryResourceProvider,
        descriptors: Mutex<HashMap<String, ResourceRef>>,
    }

    impl TestGateway {
        fn new() -> Self {
            Self {
                provider: MemoryResourceProvider::new(),
                descriptors: Mutex::new(HashMap::new()),
            }
        }

        fn remember(&self, resource: ResourceRef) -> ResourceRef {
            self.descriptors
                .lock()
                .unwrap()
                .insert(resource.ref_id.clone(), resource.clone());
            resource
        }

        fn ensure_provider(provider_id: &str) -> RuntimeResult<()> {
            if provider_id == "memory" {
                Ok(())
            } else {
                Err(RuntimeFailure::new(RuntimeError::new(
                    mutsuki_runtime_contracts::ERR_RESOURCE_NOT_FOUND,
                    "test.image.resources",
                    format!("provider.{provider_id}"),
                )))
            }
        }
    }

    impl ResourcePlanGateway for TestGateway {
        fn collect_read_plan(&self, plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
            self.provider.collect_read_plan(plan)
        }

        fn snapshot_read_plan(
            &self,
            plan: &ReadPlan,
            kind_id: &str,
            schema: &str,
        ) -> RuntimeResult<SnapshotDescriptor> {
            self.provider.snapshot_read_plan(plan, kind_id, schema)
        }

        fn open_stream_plan(&self, plan: &ReadPlan) -> RuntimeResult<StreamPlan> {
            self.provider.open_stream_plan(plan)
        }

        fn execute_export_plan(&self, plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
            self.provider.execute_export_plan(plan)
        }

        fn commit_write_plan(
            &self,
            plan: &WritePlan,
            bytes: Vec<u8>,
        ) -> RuntimeResult<PlanReceipt> {
            self.provider.commit_write_plan(plan, bytes)
        }

        fn execute_command_plan(&self, plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
            self.provider.execute_command_plan(plan)
        }

        fn execute_command_batch(&self, batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
            self.provider.execute_command_batch(batch)
        }

        fn execute_saga_plan(&self, saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
            self.provider.execute_saga_plan(saga)
        }
    }

    impl ResourceRegistryGateway for TestGateway {
        fn open_resource_descriptor(&self, ref_id: &str) -> RuntimeResult<ResourceRef> {
            self.descriptors
                .lock()
                .unwrap()
                .get(ref_id)
                .cloned()
                .ok_or_else(|| {
                    RuntimeFailure::new(RuntimeError::new(
                        mutsuki_runtime_contracts::ERR_RESOURCE_NOT_FOUND,
                        "test.image.resources",
                        format!("resource.{ref_id}"),
                    ))
                })
        }

        fn create_blob_resource(
            &self,
            provider_id: &str,
            schema: &str,
            bytes: Vec<u8>,
        ) -> RuntimeResult<ResourceRef> {
            Self::ensure_provider(provider_id)?;
            let resource =
                ResourceProviderGateway::create_blob_resource(&self.provider, schema, bytes)?;
            Ok(self.remember(resource))
        }

        fn create_cow_state_resource(
            &self,
            provider_id: &str,
            kind_id: &str,
            schema: &str,
            bytes: Vec<u8>,
        ) -> RuntimeResult<ResourceRef> {
            Self::ensure_provider(provider_id)?;
            let resource = ResourceProviderGateway::create_cow_state_resource(
                &self.provider,
                kind_id,
                schema,
                bytes,
            )?;
            Ok(self.remember(resource))
        }

        fn create_capability_resource(
            &self,
            provider_id: &str,
            kind_id: &str,
            schema: &str,
        ) -> RuntimeResult<ResourceRef> {
            Self::ensure_provider(provider_id)?;
            let resource = ResourceProviderGateway::create_capability_resource(
                &self.provider,
                kind_id,
                schema,
            )?;
            Ok(self.remember(resource))
        }
    }

    #[test]
    fn config_rejects_missing_provider_fonts_and_relative_paths() {
        let config = SkiaRenderConfig {
            output_provider_id: String::new(),
            font_files: Vec::new(),
        };
        assert!(config.validate().is_err());
        let config = SkiaRenderConfig {
            output_provider_id: "memory".into(),
            font_files: vec![PathBuf::from("font.ttf")],
        };
        assert!(config.validate().is_err());
        assert_eq!(
            manifest().provides.protocol_classes.get(RENDER),
            Some(&ProtocolClass::Effect)
        );
    }

    #[test]
    fn image_fit_preserves_cover_and_contain_geometry() {
        let dst = Rect::from_xywh(0.0, 0.0, 100.0, 100.0);
        let (cover_src, cover_dst) = fitted_rects(200.0, 100.0, dst, ImageFit::Cover);
        assert_eq!(cover_src, Rect::from_xywh(50.0, 0.0, 100.0, 100.0));
        assert_eq!(cover_dst, dst);
        let (contain_src, contain_dst) = fitted_rects(200.0, 100.0, dst, ImageFit::Contain);
        assert_eq!(contain_src, Rect::from_xywh(0.0, 0.0, 200.0, 100.0));
        assert_eq!(contain_dst, Rect::from_xywh(0.0, 25.0, 100.0, 50.0));
    }

    #[test]
    fn real_raster_render_produces_readable_png_and_rejects_missing_glyph() {
        let resources = Arc::new(TestGateway::new());
        let font =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fonts/NotoSansSC-Test.ttf");
        let runner = SkiaRenderRunner::launch(
            SkiaRenderConfig {
                output_provider_id: "memory".into(),
                font_files: vec![font],
            },
            resources.clone(),
        )
        .unwrap();
        let source = resources
            .create_blob_resource("memory", "image/png", fixture_png())
            .unwrap();
        let scene = behavior_scene(source, "米画师 Painter Painter Painter Painter");
        let task = render_task("render-ok", scene);
        let result = runner.run_task(&task).unwrap();
        let response: ImageRenderResponse = serde_json::from_value(result.output.unwrap()).unwrap();
        assert_eq!((response.width, response.height), (320, 180));
        assert_eq!(response.resource.schema, PNG_SCHEMA);
        let bytes = resources
            .collect_read_plan(&ReadPlan {
                plan_id: "read-rendered".into(),
                resource: response.resource,
                operation: "collect".into(),
                args: Value::Null,
            })
            .unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (320, 180));
        assert!(bytes.len() <= MAX_OUTPUT_BYTES);
        let pixels = decoded.to_rgba8();
        assert_eq!(pixels.get_pixel(0, 0).0[3], 0);
        assert!((225..=232).contains(&pixels.get_pixel(80, 45).0[3]));
        assert_ne!(pixels.get_pixel(185, 50), pixels.get_pixel(280, 50));
        assert!(pixels.get_pixel(165, 50).0[3] > 0);
        assert!((110..134).any(|y| (20..300).any(|x| pixels.get_pixel(x, y).0[3] > 0)));
        assert!(
            (136..170).any(|y| (20..300).any(|x| pixels.get_pixel(x, y).0[3] > 0)),
            "wrapped text must paint a second line"
        );

        let missing = runner
            .run_task(&render_task(
                "render-missing-glyph",
                behavior_scene(
                    resources
                        .create_blob_resource("memory", "image/png", fixture_png())
                        .unwrap(),
                    "🚀",
                ),
            ))
            .unwrap_err();
        assert!(missing.route.contains("text.glyph_missing"));
    }

    #[test]
    fn batch_entries_isolate_invalid_scene_and_unknown_output_provider() {
        let resources = Arc::new(TestGateway::new());
        let font =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fonts/NotoSansSC-Test.ttf");
        let mut runner = SkiaRenderRunner::launch(
            SkiaRenderConfig {
                output_provider_id: "memory".into(),
                font_files: vec![font.clone()],
            },
            resources.clone(),
        )
        .unwrap();
        let good = render_task(
            "good",
            behavior_scene(
                resources
                    .create_blob_resource("memory", "image/png", fixture_png())
                    .unwrap(),
                "Painter",
            ),
        );
        let mut invalid_scene = behavior_scene(
            resources
                .create_blob_resource("memory", "image/png", fixture_png())
                .unwrap(),
            "Painter",
        );
        invalid_scene.width = 0;
        let invalid = render_task("invalid", invalid_scene);
        let completion = runner
            .run_batch(
                RunnerContext::new(1, 1, "image-test", None, "image-test-invocation"),
                work_batch(vec![good, invalid]),
            )
            .unwrap();
        assert!(completion.results[0].result.is_some());
        assert!(completion.results[1].error.is_some());

        let unknown = SkiaRenderRunner::launch(
            SkiaRenderConfig {
                output_provider_id: "unknown".into(),
                font_files: vec![font],
            },
            resources.clone(),
        )
        .unwrap();
        let error = unknown
            .run_task(&render_task(
                "unknown-provider",
                behavior_scene(
                    resources
                        .create_blob_resource("memory", "image/png", fixture_png())
                        .unwrap(),
                    "Painter",
                ),
            ))
            .unwrap_err();
        assert!(error.route.contains("output.provider"));
    }

    fn behavior_scene(source: ResourceRef, text: &str) -> mutsuki_protocol_image::ImageScene {
        mutsuki_protocol_image::ImageScene {
            width: 320,
            height: 180,
            background: Rgba::TRANSPARENT,
            nodes: vec![
                SceneNode::Image {
                    bounds: SceneRect {
                        x: 0.0,
                        y: 0.0,
                        width: 160.0,
                        height: 90.0,
                    },
                    source: Box::new(source),
                    fit: ImageFit::Cover,
                    corner_radius: 18.0,
                    opacity: 0.9,
                    effects: Vec::new(),
                },
                SceneNode::Rect {
                    bounds: SceneRect {
                        x: 170.0,
                        y: 20.0,
                        width: 120.0,
                        height: 60.0,
                    },
                    corner_radius: 12.0,
                    fill: Fill::LinearGradient {
                        start: Point { x: 170.0, y: 20.0 },
                        end: Point { x: 290.0, y: 80.0 },
                        stops: vec![
                            mutsuki_protocol_image::GradientStop {
                                offset: 0.0,
                                color: Rgba {
                                    red: 255,
                                    green: 40,
                                    blue: 80,
                                    alpha: 220,
                                },
                            },
                            mutsuki_protocol_image::GradientStop {
                                offset: 1.0,
                                color: Rgba {
                                    red: 30,
                                    green: 40,
                                    blue: 180,
                                    alpha: 220,
                                },
                            },
                        ],
                    },
                    opacity: 1.0,
                    effects: vec![
                        SceneEffect::DropShadow {
                            offset_x: 0.0,
                            offset_y: 4.0,
                            sigma_x: 6.0,
                            sigma_y: 6.0,
                            color: Rgba {
                                red: 0,
                                green: 0,
                                blue: 0,
                                alpha: 160,
                            },
                        },
                        SceneEffect::GaussianBlur {
                            sigma_x: 2.0,
                            sigma_y: 2.0,
                        },
                    ],
                },
                SceneNode::Text {
                    bounds: SceneRect {
                        x: 20.0,
                        y: 110.0,
                        width: 280.0,
                        height: 60.0,
                    },
                    text: text.into(),
                    font_families: vec!["Noto Sans SC".into()],
                    font_size: 24.0,
                    font_weight: 600,
                    line_height: 1.1,
                    align: TextAlign::Center,
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
        }
    }

    fn fixture_png() -> Vec<u8> {
        let pixels = [120_u8, 180, 220, 255].repeat(64 * 32);
        let mut bytes = Vec::new();
        PngEncoder::new(&mut bytes)
            .write_image(&pixels, 64, 32, ColorType::Rgba8.into())
            .unwrap();
        bytes
    }

    fn render_task(id: &str, scene: mutsuki_protocol_image::ImageScene) -> Task {
        Task::new(
            id,
            RENDER,
            serde_json::to_value(ImageRenderRequest { scene }).unwrap(),
        )
    }

    fn work_batch(tasks: Vec<Task>) -> WorkBatch {
        let entries = tasks
            .iter()
            .enumerate()
            .map(|(index, task)| BatchEntry {
                entry_id: task.task_id.clone(),
                task_id: task.task_id.clone(),
                trace_id: None,
                parent_id: None,
                payload_index: index,
                resource_requirement_indices: Vec::new(),
                cancel_index: Some(index),
                deadline_tick: None,
                priority: 0,
                lane: DispatchLane::Normal,
                ordering: OrderingRequirement::None,
            })
            .collect();
        WorkBatch {
            batch_id: "image-render-batch".into(),
            tick_id: "tick-1".into(),
            batch_key: RUNNER_ID.into(),
            entries,
            payload: BatchPayload::from_local_tasks(tasks),
            resource_plan: WorkResourcePlan::empty(),
            task_leases: Vec::new(),
        }
    }
}
