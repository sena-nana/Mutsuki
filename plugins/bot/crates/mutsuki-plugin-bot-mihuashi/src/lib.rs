use std::sync::Arc;

use mutsuki_bot_link_parser::MAX_LINK_CARD_MEDIA_BYTES;
use mutsuki_bot_protocol::{
    BOT_MESSAGE_SEND_PROTOCOL_ID, BotExtMap, BotMessage, BotTarget, MessageSegment,
};
use mutsuki_protocol_browser::{
    BrowserSnapshot, BrowserSnapshotRequest, BrowserWaitMode, SNAPSHOT, SNAPSHOT_SCHEMA,
};
use mutsuki_protocol_http::{HttpRequest, HttpResponse, REQUEST as HTTP_REQUEST};
use mutsuki_protocol_image::{
    CARD_RENDER, CardGradient, CardRenderRequest, ImageRenderResponse, Rgba,
};
use mutsuki_runtime_contracts::{
    ExecutionClass, ProtocolClass, ReadPlan, ResourceRef, RunnerDescriptor, RunnerPurity,
    RunnerResult, RuntimeError, ScalarValue, Task, TaskOutcome,
};
use mutsuki_runtime_core::{Runner, RuntimeFailure, RuntimeResult};
use mutsuki_runtime_sdk::{
    AsyncRunnerContext, PluginBuilder, ProtocolDescriptorBuilder, ResourceRegistryGateway,
    RunnerDescriptorBuilder, RuntimeClientRef, TaskAwaitRunnerAdapter,
};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

pub const PLUGIN_ID: &str = "mutsuki.bot.mihuashi";
pub const RUNNER_ID: &str = "mutsuki.bot.mihuashi.runner";
pub const LINK_RESOLVE: &str = "mutsuki.bot.mihuashi.link/resolve@1";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MihuashiResolveRequest {
    pub url: String,
    pub target: BotTarget,
    pub outbound_binding: String,
    pub selector: String,
    pub timeout_ms: u64,
}

pub fn runner(
    client: RuntimeClientRef,
    resources: Arc<dyn ResourceRegistryGateway>,
    media_provider_id: String,
) -> Box<dyn Runner> {
    let descriptor = descriptor();
    let factory = Box::new(
        move |ctx: mutsuki_runtime_sdk::AsyncRunnerContext, task: Task| {
            let resources = resources.clone();
            let media_provider_id = media_provider_id.clone();
            Box::pin(async move { run_task(ctx, task, resources, media_provider_id).await })
                as std::pin::Pin<
                    Box<dyn std::future::Future<Output = RuntimeResult<RunnerResult>> + Send>,
                >
        },
    );
    Box::new(TaskAwaitRunnerAdapter::new(descriptor, client, factory).with_self_call_policy(false))
}

async fn run_task(
    ctx: AsyncRunnerContext,
    task: Task,
    resources: Arc<dyn ResourceRegistryGateway>,
    media_provider_id: String,
) -> RuntimeResult<RunnerResult> {
    let request: MihuashiResolveRequest =
        serde_json::from_value(task.payload.clone().into()).map_err(|error| fail(&task, error))?;
    ensure_mihuashi_url(&request.url).map_err(|error| fail(&task, error))?;
    let snapshot = acquire_profile_snapshot(
        &ctx,
        &task,
        resources.as_ref(),
        &media_provider_id,
        &request,
    )
    .await?;
    let card =
        parse_profile(&snapshot.html, &snapshot.final_url).map_err(|error| fail(&task, error))?;
    let image = fetch_profile_image(&ctx, &task, card.2.as_deref()).await?;
    let rendered = render_profile_card(&ctx, &task, &snapshot.final_url, card, image).await?;
    let message = BotMessage {
        message_id: None,
        target: request.target,
        sender: None,
        segments: vec![
            MessageSegment::Image {
                resource: rendered.resource,
            },
            MessageSegment::Text {
                text: snapshot.final_url,
            },
        ],
        reply_to: None,
        time_ms: None,
        ext: BotExtMap::new(),
    };
    let mut outbound = Task::new(
        format!("{}:notify", task.task_id),
        BOT_MESSAGE_SEND_PROTOCOL_ID,
        serde_json::to_value(message).map_err(|error| fail(&task, error))?,
    );
    outbound.target_binding_id = Some(request.outbound_binding);
    let mut result = RunnerResult::completed(task.task_id);
    result.tasks.push(outbound);
    Ok(result)
}

async fn acquire_profile_snapshot(
    ctx: &AsyncRunnerContext,
    task: &Task,
    resources: &dyn ResourceRegistryGateway,
    media_provider_id: &str,
    request: &MihuashiResolveRequest,
) -> RuntimeResult<BrowserSnapshot> {
    let output = resources.create_cow_state_resource(
        media_provider_id,
        "mutsuki.browser.snapshot.output",
        SNAPSHOT_SCHEMA,
        Vec::new(),
    )?;
    let outcome = ctx
        .call_raw(
            SNAPSHOT,
            serde_json::to_value(BrowserSnapshotRequest {
                url: request.url.clone(),
                output_resource: output.clone(),
                wait_mode: BrowserWaitMode::Selector,
                selector: Some(request.selector.clone()),
                timeout_ms: request.timeout_ms,
            })
            .map_err(|error| fail(task, error))?,
        )
        .await?;
    if !matches!(outcome, TaskOutcome::Completed { .. }) {
        return Err(fail(task, "browser snapshot child task failed"));
    }
    let latest = resources.open_resource_descriptor(&output.ref_id)?;
    let bytes = resources.collect_read_plan(&ReadPlan {
        plan_id: format!("mihuashi.snapshot.read.{}", task.task_id),
        resource: latest,
        operation: "collect".into(),
        args: Value::Null,
    })?;
    serde_json::from_slice(&bytes).map_err(|error| fail(task, error))
}

async fn fetch_profile_image(
    ctx: &AsyncRunnerContext,
    task: &Task,
    image_url: Option<&str>,
) -> RuntimeResult<Option<ResourceRef>> {
    if let Some(image_url) = image_url {
        ensure_mihuashi_url(image_url).map_err(|error| fail(task, error))?;
        let mut request = HttpRequest::get(image_url);
        request.limits.max_response_bytes = Some(MAX_LINK_CARD_MEDIA_BYTES as u64);
        let outcome = ctx
            .call_raw(
                HTTP_REQUEST,
                serde_json::to_value(request).map_err(|error| fail(task, error))?,
            )
            .await?;
        let response: HttpResponse = decode_child_output(task, outcome, "HTTP image")?;
        if !(200..300).contains(&response.metadata.status) {
            return Err(fail(
                task,
                format!("HTTP image returned status {}", response.metadata.status),
            ));
        }
        Ok(Some(response.body.ok_or_else(|| {
            fail(task, "HTTP image response body missing")
        })?))
    } else {
        Ok(None)
    }
}

async fn render_profile_card(
    ctx: &AsyncRunnerContext,
    task: &Task,
    final_url: &str,
    card: (String, String, Option<String>),
    image: Option<ResourceRef>,
) -> RuntimeResult<ImageRenderResponse> {
    let render_outcome = ctx
        .call_raw(
            CARD_RENDER,
            serde_json::to_value(CardRenderRequest {
                brand: "米画师".into(),
                title: card.0,
                description: card.1,
                url: final_url.to_owned(),
                cover: image,
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
            .map_err(|error| fail(task, error))?,
        )
        .await?;
    match render_outcome {
        TaskOutcome::Completed {
            output: Some(output),
            ..
        } => serde_json::from_value(output).map_err(|error| fail(task, error)),
        TaskOutcome::Completed { output: None, .. } => {
            Err(fail(task, "image renderer completed without output"))
        }
        _ => Err(fail(task, "image renderer child task failed")),
    }
}

fn decode_child_output<T: serde::de::DeserializeOwned>(
    task: &Task,
    outcome: TaskOutcome,
    operation: &str,
) -> RuntimeResult<T> {
    match outcome {
        TaskOutcome::Completed {
            output: Some(output),
            ..
        } => serde_json::from_value(output).map_err(|error| fail(task, error)),
        TaskOutcome::Completed { output: None, .. } => {
            Err(fail(task, format!("{operation} completed without output")))
        }
        TaskOutcome::Failed { error, .. } => Err(RuntimeFailure::new(error)),
        TaskOutcome::Cancelled { .. } => Err(fail(task, format!("{operation} cancelled"))),
        TaskOutcome::Expired { .. } => Err(fail(task, format!("{operation} expired"))),
        TaskOutcome::DeadLetter { .. } => Err(fail(task, format!("{operation} dead-lettered"))),
    }
}

fn parse_profile(html: &str, final_url: &str) -> Result<(String, String, Option<String>), String> {
    let document = Html::parse_document(html);
    let text = |selector: &str| -> Option<String> {
        let selector = Selector::parse(selector).ok()?;
        Some(
            document
                .select(&selector)
                .next()?
                .text()
                .collect::<String>()
                .trim()
                .to_owned(),
        )
    };
    let image = Selector::parse("meta[property='og:image']")
        .ok()
        .and_then(|selector| document.select(&selector).next())
        .and_then(|element| element.value().attr("content"))
        .map(ToOwned::to_owned);
    let title = text("h1")
        .or_else(|| text("title"))
        .ok_or("Mihuashi profile title missing")?;
    let description = text("main")
        .unwrap_or_else(|| "米画师画师/橱窗".into())
        .chars()
        .take(300)
        .collect();
    ensure_mihuashi_url(final_url)?;
    Ok((title, description, image))
}

#[must_use]
pub fn manifest() -> mutsuki_runtime_contracts::PluginManifest {
    let mut manifest = PluginBuilder::new(PLUGIN_ID)
        .runner_descriptor(descriptor())
        .protocol_handler(
            ProtocolDescriptorBuilder::new(LINK_RESOLVE)
                .input_schema(json!({"type":"object"}))
                .output_schema(json!({"type":"object"}))
                .error_schema(json!({"type":"object"}))
                .build(),
            RUNNER_ID,
            "orchestration",
        )
        .build()
        .manifest;
    manifest
        .provides
        .protocol_classes
        .insert(LINK_RESOLVE.into(), ProtocolClass::Effect);
    manifest
        .requires
        .push(format!("task_protocol:{CARD_RENDER}"));
    manifest
        .requires
        .push(format!("task_protocol:{HTTP_REQUEST}"));
    manifest
}
fn descriptor() -> RunnerDescriptor {
    RunnerDescriptorBuilder::new(RUNNER_ID, PLUGIN_ID)
        .accepted_protocol(LINK_RESOLVE)
        .purity(RunnerPurity::Effectful)
        .execution_class(ExecutionClass::Orchestration)
        .metadata("domain", ScalarValue::String("mihuashi".into()))
        .build()
}
fn ensure_mihuashi_url(value: &str) -> Result<(), String> {
    let url = Url::parse(value).map_err(|error| error.to_string())?;
    let host = url.host_str().unwrap_or_default();
    if url.scheme() == "https" && (host == "mihuashi.com" || host.ends_with(".mihuashi.com")) {
        Ok(())
    } else {
        Err(format!("Mihuashi domain denied: {host}"))
    }
}
fn fail(task: &Task, detail: impl std::fmt::Display) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        "mihuashi.resolve_failed",
        PLUGIN_ID,
        format!("mihuashi.{}", task.task_id),
    );
    error
        .evidence
        .insert("detail".into(), ScalarValue::String(detail.to_string()));
    RuntimeFailure::new(error)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::Duration;

    use async_trait::async_trait;
    use image::{ColorType, ImageEncoder, codecs::png::PngEncoder};
    use mutsuki_plugin_image_render::{ImageRenderConfig, ImageRenderRunner};
    use mutsuki_plugin_io_browser_chromium::{
        BrowserBackend, BrowserSnapshotRunner, ChromiumConfig,
    };
    use mutsuki_plugin_io_http_client::{
        FetchedHttpResponse, HttpEffectHandler, HttpGateway, HttpGatewayError,
    };
    use mutsuki_protocol_http::{HttpRequest, HttpResponseMetadata};
    use mutsuki_runtime_contracts::{PluginManifest, TaskBatch, WorkBatch};
    use mutsuki_runtime_sdk::map_work_batch_entries;
    use mutsuki_service_config::{ConfiguredPluginSelection, ServiceConfig};
    use mutsuki_service_control::{
        ControlCommand, ControlRequest, ControlResponse, ControlResult, TaskSubmitBatchParam,
        TaskWaitParam,
    };
    use mutsuki_service_runtime::{ServiceRuntime, ServiceRuntimeBuilder};
    use tempfile::tempdir;

    use super::*;

    struct FakeHttpGateway;

    #[async_trait]
    impl HttpGateway for FakeHttpGateway {
        async fn execute(
            &self,
            request: HttpRequest,
            _request_body: Option<Vec<u8>>,
        ) -> Result<FetchedHttpResponse, HttpGatewayError> {
            Ok(FetchedHttpResponse {
                metadata: HttpResponseMetadata {
                    status: 200,
                    final_url: request.url,
                    headers: std::collections::BTreeMap::default(),
                    body_bytes: fixture_png().len() as u64,
                    redirects_followed: 0,
                },
                body: fixture_png(),
                peak_buffered_bytes: fixture_png().len() as u64,
            })
        }
    }

    #[derive(Clone)]
    struct FakeBrowserBackend {
        snapshot: BrowserSnapshot,
    }

    impl BrowserBackend for FakeBrowserBackend {
        fn snapshot(&mut self, _: &BrowserSnapshotRequest) -> Result<BrowserSnapshot, String> {
            Ok(self.snapshot.clone())
        }
    }

    struct CaptureSendRunner {
        descriptor: RunnerDescriptor,
        resources: Arc<dyn ResourceRegistryGateway>,
        captured: CapturedSendSlot,
    }

    struct CapturedSend {
        message: BotMessage,
        png: Vec<u8>,
    }

    type CapturedSendSlot = Arc<Mutex<Option<CapturedSend>>>;

    impl Runner for CaptureSendRunner {
        fn descriptor(&self) -> &RunnerDescriptor {
            &self.descriptor
        }

        fn run_batch(
            &mut self,
            _ctx: mutsuki_runtime_contracts::RunnerContext,
            batch: WorkBatch,
        ) -> RuntimeResult<mutsuki_runtime_contracts::CompletionBatch> {
            let resources = self.resources.clone();
            let captured = self.captured.clone();
            map_work_batch_entries(&batch, move |task| {
                let message: BotMessage = serde_json::from_value(task.payload.to_value())
                    .map_err(|error| capture_error(task, error))?;
                let resource = message
                    .segments
                    .iter()
                    .find_map(|segment| match segment {
                        MessageSegment::Image { resource } => Some(resource.clone()),
                        _ => None,
                    })
                    .ok_or_else(|| capture_error(task, "rendered image segment missing"))?;
                let png = resources
                    .collect_read_plan(&ReadPlan {
                        plan_id: format!("mihuashi.capture.{}", task.task_id),
                        resource,
                        operation: "collect".into(),
                        args: Value::Null,
                    })
                    .map_err(|error| capture_error(task, error))?;
                *captured.lock().unwrap() = Some(CapturedSend { message, png });
                Ok(RunnerResult::completed(task.task_id.clone()))
            })
        }
    }

    fn capture_error(task: &Task, detail: impl std::fmt::Display) -> RuntimeError {
        let mut error = RuntimeError::new(
            "mihuashi.capture_failed",
            "mihuashi.test.bot-send",
            format!("mihuashi.capture.{}", task.task_id),
        );
        error
            .evidence
            .insert("detail".into(), ScalarValue::String(detail.to_string()));
        error
    }

    fn capture_descriptor() -> RunnerDescriptor {
        RunnerDescriptorBuilder::new("mihuashi.test.bot-send.runner", "mihuashi.test.bot-send")
            .accepted_protocol(BOT_MESSAGE_SEND_PROTOCOL_ID)
            .purity(RunnerPurity::Effectful)
            .execution_class(ExecutionClass::Io)
            .build()
    }

    fn capture_manifest() -> PluginManifest {
        let mut manifest = PluginBuilder::new("mihuashi.test.bot-send")
            .runner_descriptor(capture_descriptor())
            .protocol_handler(
                ProtocolDescriptorBuilder::new(BOT_MESSAGE_SEND_PROTOCOL_ID)
                    .input_schema(json!({"type": "object"}))
                    .output_schema(json!({"type": "object"}))
                    .error_schema(json!({"type": "object"}))
                    .build(),
                "mihuashi.test.bot-send.runner",
                "io",
            )
            .build()
            .manifest;
        manifest
            .provides
            .protocol_classes
            .insert(BOT_MESSAGE_SEND_PROTOCOL_ID.into(), ProtocolClass::Effect);
        manifest
    }

    #[test]
    fn manifest_classifies_the_effectful_protocol() {
        assert_eq!(
            manifest().provides.protocol_classes.get(LINK_RESOLVE),
            Some(&ProtocolClass::Effect)
        );
        assert!(
            manifest()
                .requires
                .contains(&format!("task_protocol:{CARD_RENDER}"))
        );
        assert!(
            manifest()
                .requires
                .contains(&format!("task_protocol:{HTTP_REQUEST}"))
        );
    }

    #[test]
    fn parses_server_rendered_fixture() {
        let html = "<html><head><meta property='og:image' content='https://img.mihuashi.com/a.jpg'></head><body><h1>Painter</h1><main>Window</main></body></html>";
        let parsed = parse_profile(html, "https://www.mihuashi.com/profiles/1").unwrap();
        assert_eq!(parsed.0, "Painter");
        assert_eq!(parsed.1, "Window");
    }

    #[tokio::test]
    async fn real_core_routes_browser_image_render_and_bot_send_closed_loop() {
        let root = tempdir().unwrap();
        let snapshot = test_snapshot();
        let captured = Arc::new(Mutex::new(None));
        let runtime = closed_loop_builder(
            test_service_config(root.path()),
            snapshot.clone(),
            captured.clone(),
        )
        .start()
        .await
        .unwrap();
        submit_and_wait(&runtime, snapshot.final_url).await;
        let CapturedSend { message, png } = wait_for_capture(&captured).await;
        let decoded = image::load_from_memory(&png).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (1200, 630));
        let MessageSegment::Image { resource } = &message.segments[0] else {
            panic!("first segment must be rendered image")
        };
        assert_eq!(resource.schema, mutsuki_protocol_image::PNG_SCHEMA);
        assert_eq!(
            message.segments[1],
            MessageSegment::Text {
                text: "https://www.mihuashi.com/profiles/1".into()
            }
        );
        runtime.shutdown().await;
    }

    fn test_snapshot() -> BrowserSnapshot {
        BrowserSnapshot {
            final_url: "https://www.mihuashi.com/profiles/1".into(),
            title: "Painter".into(),
            html: "<html><head><meta property='og:image' content='https://img.mihuashi.com/a.jpg'></head><body><h1>Painter</h1><main>Window</main></body></html>".into(),
        }
    }

    fn test_service_config(root: &std::path::Path) -> ServiceConfig {
        std::fs::create_dir_all(root.join("logs")).unwrap();
        let mut config = ServiceConfig::default();
        config.ipc.enabled = false;
        config.ipc.token = Some("mihuashi-test-token".into());
        config.observe.console = false;
        config.observe.json = false;
        config.service.home_dir = root.to_path_buf();
        config.service.data_dir = root.join("data");
        config.service.log_dir = root.join("logs");
        config.service.run_dir = root.join("run");
        config.plugins.dynamic_dirs.clear();
        config.plugins.disabled_dir = root.join("disabled");
        config.plugins.configured = [
            mutsuki_plugin_resource_memory::PLUGIN_ID,
            mutsuki_plugin_io_browser_chromium::PLUGIN_ID,
            mutsuki_plugin_io_http_client::PLUGIN_ID,
            mutsuki_plugin_image_render::PLUGIN_ID,
            PLUGIN_ID,
            "mihuashi.test.bot-send",
        ]
        .into_iter()
        .map(|id| ConfiguredPluginSelection {
            id: id.into(),
            enabled: true,
            config: Value::Null,
        })
        .collect();
        config
    }

    fn closed_loop_builder(
        config: ServiceConfig,
        snapshot: BrowserSnapshot,
        captured: CapturedSendSlot,
    ) -> ServiceRuntimeBuilder {
        let browser_config = ChromiumConfig {
            executable: std::env::current_exe().unwrap(),
            domain_allowlist: vec!["mihuashi.com".into()],
            timeout_ms: 5_000,
            max_dom_bytes: 2 * 1024 * 1024,
        };
        let font = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../std/plugins/mutsuki-plugin-image-render-takumi/tests/fonts/NotoSansSC-Test.ttf")
            .canonicalize()
            .unwrap();
        let memory_manifest = mutsuki_plugin_resource_memory::loaded_plugin().manifest;
        let mut image_manifest = mutsuki_plugin_image_render::manifest();
        image_manifest.requires.push(format!(
            "resource_strategy:{}",
            mutsuki_plugin_resource_memory::PLUGIN_ID
        ));
        let mut mihuashi_manifest = manifest();
        mihuashi_manifest.requires.extend([
            format!("task_protocol:{SNAPSHOT}"),
            format!(
                "resource_strategy:{}",
                mutsuki_plugin_resource_memory::PLUGIN_ID
            ),
        ]);
        let mut http_manifest = mutsuki_plugin_io_http_client::manifest();
        http_manifest.requires.push(format!(
            "resource_strategy:{}",
            mutsuki_plugin_resource_memory::PLUGIN_ID
        ));
        ServiceRuntimeBuilder::new(config)
            .register_builtin_loaded_plugin_factory(memory_manifest, || {
                Ok::<_, String>(mutsuki_plugin_resource_memory::loaded_plugin())
            })
            .register_builtin_plugin(mutsuki_plugin_io_browser_chromium::manifest())
            .register_runtime_services_runner(move |_client, resources| {
                Box::new(BrowserSnapshotRunner::with_backend(
                    browser_config.clone(),
                    resources,
                    Box::new(FakeBrowserBackend {
                        snapshot: snapshot.clone(),
                    }),
                ))
            })
            .register_builtin_plugin(http_manifest)
            .register_runtime_client_runner(mutsuki_plugin_io_http_client::facade_runner)
            .register_fallible_runtime_services_async_handler(move |_client, resources| {
                Ok::<Arc<dyn mutsuki_runtime_core::AsyncBatchHandler>, String>(Arc::new(
                    HttpEffectHandler::new(
                        Arc::new(FakeHttpGateway),
                        resources,
                        mutsuki_plugin_resource_memory::PLUGIN_ID,
                    ),
                ))
            })
            .register_builtin_plugin(image_manifest)
            .register_fallible_runtime_services_runner(move |_client, resources| {
                ImageRenderRunner::launch(
                    ImageRenderConfig {
                        output_provider_id: mutsuki_plugin_resource_memory::PLUGIN_ID.into(),
                        font_files: vec![font.clone()],
                    },
                    resources,
                )
                .map(|runner| Box::new(runner) as Box<dyn Runner>)
            })
            .register_builtin_plugin(mihuashi_manifest)
            .register_runtime_services_runner(move |client, resources| {
                runner(
                    client,
                    resources,
                    mutsuki_plugin_resource_memory::PLUGIN_ID.into(),
                )
            })
            .register_builtin_plugin(capture_manifest())
            .register_runtime_services_runner(move |_client, resources| {
                Box::new(CaptureSendRunner {
                    descriptor: capture_descriptor(),
                    resources,
                    captured: captured.clone(),
                })
            })
    }

    async fn submit_and_wait(runtime: &ServiceRuntime, url: String) {
        let control = runtime.control_handler();
        let request = MihuashiResolveRequest {
            url,
            target: BotTarget::Group {
                group_id: "group-1".into(),
            },
            outbound_binding: "qq-main".into(),
            selector: "main".into(),
            timeout_ms: 5_000,
        };
        let submit = control
            .handle(ControlRequest::new(
                runtime.control_token(),
                ControlCommand::TaskSubmitBatch(TaskSubmitBatchParam {
                    batch: TaskBatch::one(
                        "mihuashi-real-core-batch",
                        Task::new(
                            "mihuashi-real-core",
                            LINK_RESOLVE,
                            serde_json::to_value(request).unwrap(),
                        ),
                    ),
                }),
            ))
            .await;
        assert!(matches!(
            submit,
            ControlResponse::Ok(ControlResult::TaskSubmitBatch(_))
        ));
        let waited = control
            .handle(ControlRequest::new(
                runtime.control_token(),
                ControlCommand::TaskWait(TaskWaitParam {
                    ids: vec!["mihuashi-real-core".into()],
                    timeout_ms: 10_000,
                }),
            ))
            .await;
        let ControlResponse::Ok(ControlResult::TaskWait(waited)) = waited else {
            panic!("wait failed: {waited:?}");
        };
        assert!(!waited.timed_out);
        assert_eq!(waited.outcomes[0].status, "completed");
    }

    async fn wait_for_capture(captured: &CapturedSendSlot) -> CapturedSend {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while captured.lock().unwrap().is_none() {
            assert!(tokio::time::Instant::now() < deadline, "Bot send timed out");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        captured.lock().unwrap().take().unwrap()
    }

    fn fixture_png() -> Vec<u8> {
        let pixels = [220_u8, 160, 180, 255].repeat(64 * 64);
        let mut bytes = Vec::new();
        PngEncoder::new(&mut bytes)
            .write_image(&pixels, 64, 64, ColorType::Rgba8.into())
            .unwrap();
        bytes
    }
}
