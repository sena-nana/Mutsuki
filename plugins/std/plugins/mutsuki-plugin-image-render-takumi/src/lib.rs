//! Single image render backend (Takumi): compose / card / QR → PNG ResourceRef.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;

use html_escape::encode_text;
use image::ImageFormat;
use mutsuki_protocol_image::{
    CARD_RENDER, COMPOSE, CardRenderRequest, ComposeRenderRequest, ImageRenderResponse, PNG_SCHEMA,
    PROTOCOL_IDS, QR_RENDER, QrRenderRequest, validate_compose,
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
use qrcode::QrCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use takumi::prelude::{
    FontResource, Fonts, FromHtmlOptions, ImageSource, OutputFormat, RenderOptions, StyleSheet,
    Viewport,
};
use takumi::{from_html, render, write_image};

pub const PLUGIN_ID: &str = "mutsuki.std.image.render";
pub const RUNNER_ID: &str = "mutsuki.std.image.render.runner";
pub const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const CARD_WIDTH: u32 = 1200;
const CARD_HEIGHT: u32 = 630;
const COVER_SRC: &str = "mutsuki-card-cover";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageRenderConfig {
    pub output_provider_id: String,
    pub font_files: Vec<PathBuf>,
}

impl ImageRenderConfig {
    /// Validates provider selection and absolute font file paths.
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

pub struct ImageRenderRunner {
    descriptor: RunnerDescriptor,
    output_provider_id: String,
    fonts: Fonts,
    font_families: Vec<String>,
    resources: Arc<dyn ResourceRegistryGateway>,
}

impl ImageRenderRunner {
    /// Loads configured fonts; never reads system fonts.
    pub fn launch(
        config: ImageRenderConfig,
        resources: Arc<dyn ResourceRegistryGateway>,
    ) -> Result<Self, String> {
        config.validate()?;
        let mut fonts = Fonts::default();
        let mut font_families = Vec::with_capacity(config.font_files.len());
        for path in &config.font_files {
            let bytes = std::fs::read(path)
                .map_err(|error| format!("failed to read font {}: {error}", path.display()))?;
            let registered = fonts.register(FontResource::new(bytes)).map_err(|error| {
                format!("invalid or unsupported font file {}: {error}", path.display())
            })?;
            if registered.is_empty() {
                return Err(format!("font registered no families: {}", path.display()));
            }
            for family in registered {
                if family.name.trim().is_empty() {
                    return Err(format!("font has no family name: {}", path.display()));
                }
                if !font_families.contains(&family.name) {
                    font_families.push(family.name);
                }
            }
        }
        Ok(Self {
            descriptor: runner_descriptor(),
            output_provider_id: config.output_provider_id,
            fonts,
            font_families,
            resources,
        })
    }

    fn run_task(&self, task: &Task) -> Result<RunnerResult, RuntimeError> {
        let (bytes, width, height) = match task.protocol_id.as_str() {
            COMPOSE => {
                let request: ComposeRenderRequest = serde_json::from_value(task.payload.to_value())
                    .map_err(|error| render_error(task, "request.invalid", error.to_string()))?;
                validate_compose(&request)
                    .map_err(|error| render_error(task, error.code, error.detail))?;
                let bytes = self.render_html(
                    task,
                    &request.html,
                    request.width,
                    request.height,
                    request.stylesheet.as_deref(),
                    HashMap::new(),
                )?;
                (bytes, request.width, request.height)
            }
            CARD_RENDER => {
                let request: CardRenderRequest = serde_json::from_value(task.payload.to_value())
                    .map_err(|error| render_error(task, "request.invalid", error.to_string()))?;
                self.render_card(task, &request)?
            }
            QR_RENDER => {
                let request: QrRenderRequest = serde_json::from_value(task.payload.to_value())
                    .map_err(|error| render_error(task, "request.invalid", error.to_string()))?;
                render_qr(task, &request)?
            }
            protocol_id => {
                return Err(render_error(
                    task,
                    "request.protocol",
                    format!("unsupported image protocol: {protocol_id}"),
                ));
            }
        };
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
            width,
            height,
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

    fn render_card(
        &self,
        task: &Task,
        request: &CardRenderRequest,
    ) -> Result<(Vec<u8>, u32, u32), RuntimeError> {
        let mut images = HashMap::new();
        let background = if let Some(cover) = &request.cover {
            let bytes = self.load_image_bytes(task, cover)?;
            let source = ImageSource::from_bytes(&bytes)
                .map_err(|error| render_error(task, "card.cover.decode", error.to_string()))?;
            images.insert(Arc::<str>::from(COVER_SRC), source);
            format!(
                r#"<img src="{COVER_SRC}" style="position:absolute;inset:0;width:100%;height:100%;object-fit:cover;" />"#
            )
        } else {
            let start = css_rgba(request.fallback_gradient.start);
            let end = css_rgba(request.fallback_gradient.end);
            format!(
                r#"<div style="position:absolute;inset:0;background:linear-gradient(135deg,{start},{end});"></div>"#
            )
        };
        let family = self
            .font_families
            .first()
            .map(String::as_str)
            .unwrap_or("sans-serif");
        let html = format!(
            r#"<div style="width:100%;height:100%;position:relative;overflow:hidden;font-family:'{family}',sans-serif;color:#fff;">
  {background}
  <div style="position:absolute;left:0;right:0;bottom:0;height:45%;background:linear-gradient(to bottom,rgba(12,10,18,0),rgba(12,10,18,0.75) 35%,rgba(12,10,18,0.96));"></div>
  <div style="position:absolute;left:48px;right:48px;bottom:40px;display:flex;flex-direction:column;gap:10px;">
    <div style="font-size:22px;font-weight:700;line-height:1;">{brand}</div>
    <div style="font-size:38px;font-weight:700;line-height:1.08;">{title}</div>
    <div style="font-size:20px;font-weight:400;line-height:1.1;opacity:0.92;">{description}</div>
    <div style="font-size:18px;font-weight:400;opacity:0.85;">{url}</div>
  </div>
</div>"#,
            brand = encode_text(&request.brand),
            title = encode_text(&request.title),
            description = encode_text(&request.description),
            url = encode_text(&request.url),
        );
        let bytes = self.render_html(task, &html, CARD_WIDTH, CARD_HEIGHT, None, images)?;
        Ok((bytes, CARD_WIDTH, CARD_HEIGHT))
    }

    fn load_image_bytes(
        &self,
        task: &Task,
        resource: &mutsuki_runtime_contracts::ResourceRef,
    ) -> Result<Vec<u8>, RuntimeError> {
        if resource.ref_id.trim().is_empty() {
            return Err(render_error(
                task,
                "card.cover.resource",
                "image resource ref_id is empty",
            ));
        }
        if resource
            .size_hint
            .is_some_and(|size| size > MAX_IMAGE_BYTES as u64)
        {
            return Err(render_error(
                task,
                "card.cover.oversized",
                format!(
                    "image resource {} exceeds {MAX_IMAGE_BYTES} bytes",
                    resource.ref_id
                ),
            ));
        }
        let bytes = self
            .resources
            .collect_read_plan(&ReadPlan {
                plan_id: format!("image-cover-{}-{}", task.task_id, resource.ref_id),
                resource: resource.clone(),
                operation: "collect".into(),
                args: Value::Null,
            })
            .map_err(|error| render_error(task, "card.cover.read", error.to_string()))?;
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(render_error(
                task,
                "card.cover.oversized",
                format!(
                    "image resource {} exceeds {MAX_IMAGE_BYTES} bytes",
                    resource.ref_id
                ),
            ));
        }
        Ok(bytes)
    }

    fn render_html(
        &self,
        task: &Task,
        html: &str,
        width: u32,
        height: u32,
        stylesheet: Option<&str>,
        images: HashMap<Arc<str>, ImageSource>,
    ) -> Result<Vec<u8>, RuntimeError> {
        let node = from_html(html, FromHtmlOptions::default())
            .map_err(|error| render_error(task, "html.parse", error.to_string()))?;
        let stylesheet = match stylesheet {
            Some(css) if !css.trim().is_empty() => StyleSheet::parse(css)
                .map_err(|error| render_error(task, "stylesheet.parse", error.to_string()))?,
            _ => StyleSheet::default(),
        };
        let options = RenderOptions::builder()
            .viewport(Viewport::new((width, height)))
            .node(node)
            .fonts(&self.fonts)
            .stylesheet(Arc::new(stylesheet))
            .images(images)
            .build();
        let bitmap = render(options)
            .map_err(|error| render_error(task, "raster.render", error.to_string()))?;
        let mut out = Cursor::new(Vec::new());
        write_image(&bitmap, &mut out, OutputFormat::Png)
            .map_err(|error| render_error(task, "output.png", error.to_string()))?;
        Ok(out.into_inner())
    }

    #[must_use]
    pub fn registered_font_families(&self) -> &[String] {
        &self.font_families
    }
}

impl Runner for ImageRenderRunner {
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

fn render_qr(task: &Task, request: &QrRenderRequest) -> Result<(Vec<u8>, u32, u32), RuntimeError> {
    if request.content.is_empty() {
        return Err(render_error(task, "qr.content", "QR content is empty"));
    }
    if request.min_dimensions == 0 || request.min_dimensions > mutsuki_protocol_image::MAX_CANVAS_EDGE
    {
        return Err(render_error(
            task,
            "qr.dimensions",
            format!(
                "QR minimum dimensions must be between 1 and {}",
                mutsuki_protocol_image::MAX_CANVAS_EDGE
            ),
        ));
    }
    let image = QrCode::new(request.content.as_bytes())
        .map_err(|error| render_error(task, "qr.encode", error.to_string()))?
        .render::<image::Luma<u8>>()
        .min_dimensions(request.min_dimensions, request.min_dimensions)
        .build();
    let (width, height) = image.dimensions();
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageLuma8(image)
        .write_to(&mut bytes, ImageFormat::Png)
        .map_err(|error| render_error(task, "qr.png", error.to_string()))?;
    Ok((bytes.into_inner(), width, height))
}

fn css_rgba(color: mutsuki_protocol_image::Rgba) -> String {
    format!(
        "rgba({},{},{},{:.3})",
        color.red,
        color.green,
        color.blue,
        f32::from(color.alpha) / 255.0
    )
}

#[must_use]
pub fn manifest() -> mutsuki_runtime_contracts::PluginManifest {
    let mut builder = PluginBuilder::new(PLUGIN_ID).runner(Box::new(ManifestOnlyRunner {
        descriptor: runner_descriptor(),
    }));
    for protocol_id in PROTOCOL_IDS {
        builder = builder.protocol_handler(protocol_descriptor(protocol_id), RUNNER_ID, "cpu");
    }
    let mut manifest = builder.build().manifest;
    for protocol_id in PROTOCOL_IDS {
        manifest
            .provides
            .protocol_classes
            .insert((*protocol_id).into(), ProtocolClass::Effect);
    }
    manifest
}

fn runner_descriptor() -> RunnerDescriptor {
    let mut builder = RunnerDescriptorBuilder::new(RUNNER_ID, PLUGIN_ID);
    for protocol_id in PROTOCOL_IDS {
        builder = builder.accepted_protocol(*protocol_id);
    }
    builder
        .purity(RunnerPurity::Effectful)
        .execution_class(ExecutionClass::Cpu)
        .batch_capability(RunnerBatchCapability {
            mode: RunnerMode::ScalarAdapter,
            side_effect: RunnerSideEffect::External,
            ..Default::default()
        })
        .metadata("standard_plugin", ScalarValue::String("image_render".into()))
        .build()
}

fn protocol_descriptor(protocol_id: &str) -> mutsuki_runtime_contracts::ProtocolDescriptor {
    ProtocolDescriptorBuilder::new(protocol_id)
        .input_schema(mutsuki_protocol_image::input_schema(protocol_id).unwrap())
        .output_schema(mutsuki_protocol_image::output_schema(protocol_id).unwrap())
        .error_schema(mutsuki_protocol_image::error_schema(protocol_id).unwrap())
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

    use image::{ColorType, GenericImageView, ImageEncoder, codecs::png::PngEncoder};
    use mutsuki_plugin_resource_memory::MemoryResourceProvider;
    use mutsuki_protocol_image::{CardGradient, Rgba};
    use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
    use mutsuki_runtime_contracts::{
        CommandPlan, ExportPlan, PlanReceipt, ResourceRef, SnapshotDescriptor, StreamPlan,
        WritePlan,
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

    fn test_font() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fonts/NotoSansSC-Test.ttf")
    }

    fn launch_ok() -> (ImageRenderRunner, Arc<TestGateway>) {
        let resources = Arc::new(TestGateway::new());
        let runner = ImageRenderRunner::launch(
            ImageRenderConfig {
                output_provider_id: "memory".into(),
                font_files: vec![test_font()],
            },
            resources.clone(),
        )
        .unwrap();
        (runner, resources)
    }

    fn output_png(resources: &TestGateway, result: RunnerResult, plan_id: &str) -> Vec<u8> {
        let response: ImageRenderResponse =
            serde_json::from_value(result.output.unwrap()).unwrap();
        resources
            .collect_read_plan(&ReadPlan {
                plan_id: plan_id.into(),
                resource: response.resource,
                operation: "collect".into(),
                args: Value::Null,
            })
            .unwrap()
    }

    fn fixture_png() -> Vec<u8> {
        let mut bytes = Vec::new();
        PngEncoder::new(&mut bytes)
            .write_image(&[240, 91, 122, 255, 91, 72, 176, 255], 2, 1, ColorType::Rgba8.into())
            .unwrap();
        bytes
    }

    #[test]
    fn config_rejects_missing_provider_fonts_and_relative_paths() {
        assert!(
            ImageRenderConfig {
                output_provider_id: String::new(),
                font_files: Vec::new(),
            }
            .validate()
            .is_err()
        );
        assert!(
            ImageRenderConfig {
                output_provider_id: "memory".into(),
                font_files: vec![PathBuf::from("font.ttf")],
            }
            .validate()
            .is_err()
        );
        for protocol_id in PROTOCOL_IDS {
            assert_eq!(
                manifest().provides.protocol_classes.get(*protocol_id),
                Some(&ProtocolClass::Effect)
            );
        }
    }

    #[test]
    fn chinese_gradient_grid_compose_renders_readable_png() {
        let (runner, resources) = launch_ok();
        let family = &runner.registered_font_families()[0];
        let html = include_str!("../tests/fixtures/chinese_gradient_grid.html").replace(
            r#"background:linear-gradient(135deg,#fff7ed,#fecaca);">"#,
            &format!(
                r#"background:linear-gradient(135deg,#fff7ed,#fecaca);font-family:'{family}',sans-serif;">"#
            ),
        );
        let result = runner
            .run_task(&Task::new(
                "compose-cn-grid",
                COMPOSE,
                serde_json::to_value(ComposeRenderRequest {
                    html,
                    width: 640,
                    height: 360,
                    stylesheet: None,
                })
                .unwrap(),
            ))
            .unwrap();
        let response: ImageRenderResponse =
            serde_json::from_value(result.output.clone().unwrap()).unwrap();
        assert_eq!((response.width, response.height), (640, 360));
        assert_eq!(response.resource.schema, PNG_SCHEMA);
        let bytes = output_png(&resources, result, "read-compose");
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (640, 360));
    }

    #[test]
    fn card_and_qr_protocols_produce_png_resources() {
        let (runner, resources) = launch_ok();
        let cover = resources
            .create_blob_resource("memory", "image/png", fixture_png())
            .unwrap();
        let card = runner
            .run_task(&Task::new(
                "card",
                CARD_RENDER,
                serde_json::to_value(CardRenderRequest {
                    brand: "米画师".into(),
                    title: "Painter".into(),
                    description: "Window".into(),
                    url: "https://www.mihuashi.com/profiles/1".into(),
                    cover: Some(cover),
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
                })
                .unwrap(),
            ))
            .unwrap();
        let card_response: ImageRenderResponse =
            serde_json::from_value(card.output.clone().unwrap()).unwrap();
        assert_eq!(
            (card_response.width, card_response.height),
            (CARD_WIDTH, CARD_HEIGHT)
        );
        assert_eq!(card_response.resource.schema, PNG_SCHEMA);
        let card_png = output_png(&resources, card, "read-card");
        assert_eq!(
            image::load_from_memory(&card_png).unwrap().dimensions(),
            (CARD_WIDTH, CARD_HEIGHT)
        );

        let qr = runner
            .run_task(&Task::new(
                "qr",
                QR_RENDER,
                serde_json::to_value(QrRenderRequest {
                    content: "https://www.bilibili.com".into(),
                    min_dimensions: 256,
                })
                .unwrap(),
            ))
            .unwrap();
        let qr_response: ImageRenderResponse =
            serde_json::from_value(qr.output.clone().unwrap()).unwrap();
        assert!(qr_response.width >= 256);
        assert_eq!(qr_response.resource.schema, PNG_SCHEMA);
    }

    #[test]
    fn empty_html_and_invalid_stylesheet_fail_loud() {
        let (runner, _) = launch_ok();
        let empty = runner
            .run_task(&Task::new(
                "empty",
                COMPOSE,
                serde_json::to_value(ComposeRenderRequest {
                    html: "   ".into(),
                    width: 100,
                    height: 100,
                    stylesheet: None,
                })
                .unwrap(),
            ))
            .unwrap_err();
        assert!(empty.route.contains("compose.html"));
        let bad_css = runner
            .run_task(&Task::new(
                "bad-css",
                COMPOSE,
                serde_json::to_value(ComposeRenderRequest {
                    html: r#"<div style="width:100%;height:100%;background:#fff;">ok</div>"#.into(),
                    width: 64,
                    height: 64,
                    stylesheet: Some("{ this is not css".into()),
                })
                .unwrap(),
            ))
            .unwrap_err();
        assert!(bad_css.route.contains("stylesheet.parse"));
    }
}
