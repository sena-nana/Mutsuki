// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::default_trait_access,
    clippy::missing_panics_doc,
    clippy::needless_pass_by_value
)]

use std::sync::Arc;

use mutsuki_bot_link_parser::MAX_LINK_CARD_MEDIA_BYTES;
use mutsuki_bot_protocol::{
    BOT_MESSAGE_SEND_PROTOCOL_ID, BotEvent, BotExtMap, BotFlowEventEnvelope, BotFlowPayload,
    BotFlowTypeRef, BotMessage, BotNodeBinding, BotNodeCatalogFragment, BotNodeDescriptor,
    BotNodeInvocation, BotNodeOutput, BotNodePortDescriptor, BotNodePortDirection, BotNodeResult,
    BotNodeRole, BotTarget, MessageSegment,
};
use mutsuki_protocol_browser::{
    BrowserSnapshot, BrowserSnapshotRequest, BrowserWaitMode, SNAPSHOT, SNAPSHOT_SCHEMA,
};
use mutsuki_protocol_http::{HttpRequest, HttpResponse, REQUEST as HTTP_REQUEST};
use mutsuki_protocol_image::{
    CARD_RENDER, CardGradient, CardLayout, CardRenderRequest, ImageRenderResponse, Rgba,
};
#[cfg(test)]
use mutsuki_runtime_contracts::{ContractSurfaceKind, SurfaceRequirement};
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

const MIHUASHI_DOMAIN: &str = "mihuashi.com";
const MEDIA_CONNECT_TIMEOUT_MS: u64 = 5_000;
const MEDIA_HEADER_TIMEOUT_MS: u64 = 10_000;
const MEDIA_IDLE_TIMEOUT_MS: u64 = 10_000;
const MEDIA_TOTAL_TIMEOUT_MS: u64 = 30_000;
const MEDIA_MAX_REDIRECTS: u8 = 5;

const ERROR_URL_NOT_ALLOWED: &str = "mihuashi.url_not_allowed";
const ERROR_REDIRECT_NOT_ALLOWED: &str = "mihuashi.redirect_not_allowed";
const ERROR_MEDIA_OVERSIZED: &str = "mihuashi.media_oversized";
const ERROR_MEDIA_TIMEOUT: &str = "mihuashi.media_timeout";

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
    let payload: Value = task.payload.clone().into();
    let invocation = serde_json::from_value::<BotNodeInvocation>(payload.clone()).ok();
    let request = match &invocation {
        Some(invocation) => {
            mihuashi_request_from_invocation(invocation).map_err(|error| fail(&task, error))?
        }
        None => serde_json::from_value(payload).map_err(|error| fail(&task, error))?,
    };
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
    let outbound_binding = request.outbound_binding.clone();
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
    if let Some(invocation) = invocation {
        return flow_message_result(&task, invocation, message);
    }
    let mut outbound = Task::new(
        format!("{}:notify", task.task_id),
        BOT_MESSAGE_SEND_PROTOCOL_ID,
        serde_json::to_value(message).map_err(|error| fail(&task, error))?,
    );
    outbound.target_binding_id = Some(outbound_binding.into());
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
    if !matches!(outcome.into_outcome(), TaskOutcome::Completed { .. }) {
        return Err(fail(task, "browser snapshot child task failed"));
    }
    let latest = resources.open_resource_descriptor(output.ref_id.as_str())?;
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
    let Some(image_url) = image_url else {
        return Ok(None);
    };
    ensure_mihuashi_url(image_url)
        .map_err(|error| fail_code(task, ERROR_URL_NOT_ALLOWED, error))?;
    let outcome = ctx
        .call_raw(
            HTTP_REQUEST,
            serde_json::to_value(media_http_request(image_url))
                .map_err(|error| fail(task, error))?,
        )
        .await?;
    let response = decode_http_image(task, outcome)?;
    if !(200..300).contains(&response.metadata.status) {
        return Err(fail(
            task,
            format!("HTTP image returned status {}", response.metadata.status),
        ));
    }
    ensure_mihuashi_url(&response.metadata.final_url)
        .map_err(|error| fail_code(task, ERROR_REDIRECT_NOT_ALLOWED, error))?;
    Ok(Some(response.body.ok_or_else(|| {
        fail(task, "HTTP image response body missing")
    })?))
}

fn media_http_request(image_url: &str) -> HttpRequest {
    let mut request = HttpRequest::get(image_url);
    request.limits.max_response_bytes = Some(MAX_LINK_CARD_MEDIA_BYTES as u64);
    request.limits.connect_timeout_ms = Some(MEDIA_CONNECT_TIMEOUT_MS);
    request.limits.header_timeout_ms = Some(MEDIA_HEADER_TIMEOUT_MS);
    request.limits.idle_timeout_ms = Some(MEDIA_IDLE_TIMEOUT_MS);
    request.limits.total_timeout_ms = Some(MEDIA_TOTAL_TIMEOUT_MS);
    request.limits.max_redirects = Some(MEDIA_MAX_REDIRECTS);
    request.limits.domain_allowlist = Some(vec![MIHUASHI_DOMAIN.into()]);
    request
}

fn decode_http_image(task: &Task, outcome: impl Into<TaskOutcome>) -> RuntimeResult<HttpResponse> {
    match outcome.into() {
        TaskOutcome::Completed {
            output: Some(output),
            ..
        } => serde_json::from_value(output).map_err(|error| fail(task, error)),
        TaskOutcome::Completed { output: None, .. } => {
            Err(fail(task, "HTTP image completed without output"))
        }
        TaskOutcome::Failed { error, .. } => Err(map_http_media_failure(task, error)),
        TaskOutcome::Cancelled { .. } | TaskOutcome::Expired { .. } => Err(fail_code(
            task,
            ERROR_MEDIA_TIMEOUT,
            "HTTP image download timed out",
        )),
        TaskOutcome::DeadLetter { .. } => Err(fail(task, "HTTP image was dead-lettered")),
    }
}

fn map_http_media_failure(task: &Task, error: RuntimeError) -> RuntimeFailure {
    let code = match error.code.as_str() {
        "http.domain_denied" | "http.https_required" | "http.invalid_url" => ERROR_URL_NOT_ALLOWED,
        "http.redirect_denied" | "http.too_many_redirects" | "http.private_address" => {
            ERROR_REDIRECT_NOT_ALLOWED
        }
        "http.body_too_large" => ERROR_MEDIA_OVERSIZED,
        "http.header_timeout" | "http.idle_timeout" | "http.total_timeout" => ERROR_MEDIA_TIMEOUT,
        _ => "mihuashi.resolve_failed",
    };
    let mut mapped = RuntimeError::new(code, PLUGIN_ID, format!("mihuashi.{}", task.task_id));
    mapped.evidence = error.evidence;
    mapped
        .evidence
        .insert("http_code".into(), ScalarValue::String(error.code.clone()));
    mapped
        .evidence
        .insert("detail".into(), ScalarValue::String(error.route.clone()));
    RuntimeFailure::new(mapped)
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
                layout: mihuashi_layout(final_url),
                kicker: mihuashi_kicker(final_url).into(),
                ..CardRenderRequest::default()
            })
            .map_err(|error| fail(task, error))?,
        )
        .await?
        .into_outcome();
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
        .extension(
            mihuashi_node_catalog()
                .into_plugin_extension()
                .expect("mihuashi node catalog serializes"),
        )
        .build()
        .manifest;
    manifest
        .provides
        .protocol_classes
        .insert(LINK_RESOLVE.into(), ProtocolClass::Effect);
    manifest
}
fn descriptor() -> RunnerDescriptor {
    RunnerDescriptorBuilder::new(RUNNER_ID, PLUGIN_ID)
        .accepted_protocol(LINK_RESOLVE)
        .requires_protocol(SNAPSHOT)
        .requires_protocol(HTTP_REQUEST)
        .requires_protocol(CARD_RENDER)
        .purity(RunnerPurity::Effectful)
        .execution_class(ExecutionClass::Orchestration)
        .metadata("domain", ScalarValue::String("mihuashi".into()))
        .build()
}

fn mihuashi_node_catalog() -> BotNodeCatalogFragment {
    BotNodeCatalogFragment {
        nodes: vec![BotNodeDescriptor {
            node_type_id: "mutsuki.bot.mihuashi.resolve".into(),
            version: 1,
            title: "米画师链接".into(),
            category: "链接".into(),
            role: BotNodeRole::Processor,
            binding: Some(BotNodeBinding {
                binding_id: format!("binding:{LINK_RESOLVE}"),
                protocol_id: LINK_RESOLVE.into(),
                runner_hint: Some(RUNNER_ID.into()),
            }),
            ports: vec![
                BotNodePortDescriptor {
                    port_id: "event".into(),
                    title: "事件".into(),
                    direction: BotNodePortDirection::Input,
                    event_type: BotFlowTypeRef::new("mutsuki.bot.event", 1),
                    required: true,
                },
                BotNodePortDescriptor {
                    port_id: "message".into(),
                    title: "发送消息".into(),
                    direction: BotNodePortDirection::Output,
                    event_type: BotFlowTypeRef::new("mutsuki.bot.message.send", 1),
                    required: false,
                },
            ],
            config_schema: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "url": {"type": "string", "title": "米画师链接"}
                }
            }),
        }],
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct MihuashiFlowConfig {
    url: Option<String>,
    outbound_binding: Option<String>,
}

fn mihuashi_request_from_invocation(
    invocation: &BotNodeInvocation,
) -> Result<MihuashiResolveRequest, String> {
    let config: MihuashiFlowConfig =
        serde_json::from_value(invocation.config.clone()).map_err(|error| error.to_string())?;
    let event: BotEvent = serde_json::from_value(invocation.input.payload.value.clone())
        .map_err(|error| error.to_string())?;
    let url = config
        .url
        .filter(|value| !value.is_empty())
        .or_else(|| first_http_url(&event))
        .ok_or_else(|| "mihuashi url is missing".to_string())?;
    Ok(MihuashiResolveRequest {
        url,
        target: event.target,
        outbound_binding: config.outbound_binding.unwrap_or_default(),
        selector: String::new(),
        timeout_ms: 30_000,
    })
}

fn first_http_url(event: &BotEvent) -> Option<String> {
    event.message.as_ref().and_then(|message| {
        message.segments.iter().find_map(|segment| match segment {
            MessageSegment::Text { text } => text
                .split_whitespace()
                .find(|part| part.starts_with("http://") || part.starts_with("https://"))
                .map(str::to_owned),
            _ => None,
        })
    })
}

fn flow_message_result(
    task: &Task,
    invocation: BotNodeInvocation,
    message: BotMessage,
) -> RuntimeResult<RunnerResult> {
    let output = BotNodeOutput {
        port_id: "message".into(),
        event: BotFlowEventEnvelope {
            event_id: invocation.input.event_id.clone(),
            protocol_id: BOT_MESSAGE_SEND_PROTOCOL_ID.into(),
            payload: BotFlowPayload {
                event_type: BotFlowTypeRef::new("mutsuki.bot.message.send", 1),
                value: serde_json::to_value(message).map_err(|error| fail(task, error))?,
            },
            context: invocation.input.context.clone(),
            trace_id: invocation.input.trace_id.clone(),
            correlation_id: invocation.input.correlation_id.clone(),
        },
    };
    let mut result = RunnerResult::completed(task.task_id.clone());
    result.output = Some(
        serde_json::to_value(BotNodeResult {
            outputs: vec![output],
            metadata: Default::default(),
        })
        .map_err(|error| fail(task, error))?,
    );
    Ok(result)
}

fn mihuashi_layout(url: &str) -> CardLayout {
    if Url::parse(url)
        .ok()
        .is_some_and(|parsed| parsed.path().contains("/profiles/"))
    {
        CardLayout::Profile
    } else {
        CardLayout::Art
    }
}

fn mihuashi_kicker(url: &str) -> &'static str {
    if matches!(mihuashi_layout(url), CardLayout::Profile) {
        "画师"
    } else {
        "作品"
    }
}

fn ensure_mihuashi_url(value: &str) -> Result<(), String> {
    let url = Url::parse(value).map_err(|error| error.to_string())?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Mihuashi URL userinfo is not allowed".into());
    }
    let host = url.host_str().unwrap_or_default();
    if url.scheme() == "https" && (host == MIHUASHI_DOMAIN || host.ends_with(".mihuashi.com")) {
        Ok(())
    } else {
        Err(format!("Mihuashi domain denied: {host}"))
    }
}
fn fail(task: &Task, detail: impl std::fmt::Display) -> RuntimeFailure {
    fail_code(task, "mihuashi.resolve_failed", detail)
}

fn fail_code(task: &Task, code: &str, detail: impl std::fmt::Display) -> RuntimeFailure {
    let mut error = RuntimeError::new(code, PLUGIN_ID, format!("mihuashi.{}", task.task_id));
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
    use mutsuki_protocol_http::{HttpErrorCode, HttpRequest, HttpResponseMetadata};
    use mutsuki_runtime_contracts::{PluginManifest, TaskBatch, WorkBatch};
    use mutsuki_runtime_sdk::map_work_batch_entries;
    use mutsuki_service_config::{ConfiguredPluginSelection, ServiceConfig};
    use mutsuki_service_control::{
        ControlCommand, ControlRequest, ControlResponse, ControlResult, TaskSubmitBatchParam,
        TaskWaitParam, TaskWaitResponse,
    };
    use mutsuki_service_runtime::{ServiceRuntime, ServiceRuntimeBuilder};
    use tempfile::tempdir;

    use super::*;

    struct ScriptedHttpGateway {
        final_url: Option<String>,
        error: Option<HttpGatewayError>,
        captured: Arc<Mutex<Option<HttpRequest>>>,
    }

    #[async_trait]
    impl HttpGateway for ScriptedHttpGateway {
        async fn execute(
            &self,
            request: HttpRequest,
            _request_body: Option<Vec<u8>>,
        ) -> Result<FetchedHttpResponse, HttpGatewayError> {
            *self.captured.lock().unwrap() = Some(request.clone());
            if let Some(error) = self.error.clone() {
                return Err(error);
            }
            let final_url = self
                .final_url
                .clone()
                .unwrap_or_else(|| request.url.clone());
            Ok(FetchedHttpResponse {
                metadata: HttpResponseMetadata {
                    status: 200,
                    final_url,
                    headers: std::collections::BTreeMap::default(),
                    body_bytes: fixture_png().len() as u64,
                    redirects_followed: u8::from(self.final_url.is_some()),
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
                .contains(&SurfaceRequirement::task_protocol(CARD_RENDER))
        );
        assert!(
            manifest()
                .requires
                .contains(&SurfaceRequirement::task_protocol(HTTP_REQUEST))
        );
    }

    #[test]
    fn parses_server_rendered_fixture() {
        let html = "<html><head><meta property='og:image' content='https://img.mihuashi.com/a.jpg'></head><body><h1>Painter</h1><main>Window</main></body></html>";
        let parsed = parse_profile(html, "https://www.mihuashi.com/profiles/1").unwrap();
        assert_eq!(parsed.0, "Painter");
        assert_eq!(parsed.1, "Window");
        assert_eq!(
            mihuashi_layout("https://www.mihuashi.com/profiles/1"),
            CardLayout::Profile
        );
        assert_eq!(
            mihuashi_layout("https://www.mihuashi.com/artworks/9"),
            CardLayout::Art
        );
    }

    #[test]
    fn media_http_request_pins_mihuashi_allowlist_budgets_and_deadlines() {
        let request = media_http_request("https://img.mihuashi.com/a.jpg");
        assert_eq!(
            request.limits.max_response_bytes,
            Some(MAX_LINK_CARD_MEDIA_BYTES as u64)
        );
        assert_eq!(
            request.limits.connect_timeout_ms,
            Some(MEDIA_CONNECT_TIMEOUT_MS)
        );
        assert_eq!(
            request.limits.header_timeout_ms,
            Some(MEDIA_HEADER_TIMEOUT_MS)
        );
        assert_eq!(request.limits.idle_timeout_ms, Some(MEDIA_IDLE_TIMEOUT_MS));
        assert_eq!(
            request.limits.total_timeout_ms,
            Some(MEDIA_TOTAL_TIMEOUT_MS)
        );
        assert_eq!(request.limits.max_redirects, Some(MEDIA_MAX_REDIRECTS));
        assert_eq!(
            request.limits.domain_allowlist.as_deref(),
            Some([MIHUASHI_DOMAIN.to_owned()].as_slice())
        );
    }

    #[test]
    fn ensure_mihuashi_url_rejects_http_foreign_hosts_and_userinfo() {
        assert!(ensure_mihuashi_url("https://www.mihuashi.com/x").is_ok());
        assert!(ensure_mihuashi_url("http://www.mihuashi.com/x").is_err());
        assert!(ensure_mihuashi_url("https://evil.example/x").is_err());
        assert!(ensure_mihuashi_url("https://user:pass@www.mihuashi.com/x").is_err());
    }

    #[test]
    fn http_failures_map_to_distinct_mihuashi_media_codes() {
        let task = Task::new("t1", LINK_RESOLVE, json!({}));
        let cases = [
            ("http.domain_denied", ERROR_URL_NOT_ALLOWED),
            ("http.redirect_denied", ERROR_REDIRECT_NOT_ALLOWED),
            ("http.too_many_redirects", ERROR_REDIRECT_NOT_ALLOWED),
            ("http.private_address", ERROR_REDIRECT_NOT_ALLOWED),
            ("http.body_too_large", ERROR_MEDIA_OVERSIZED),
            ("http.header_timeout", ERROR_MEDIA_TIMEOUT),
            ("http.idle_timeout", ERROR_MEDIA_TIMEOUT),
            ("http.total_timeout", ERROR_MEDIA_TIMEOUT),
        ];
        for (http_code, expected) in cases {
            let error = RuntimeError::new(http_code, "runtime.io_http_client", "boom");
            let mapped = map_http_media_failure(&task, error);
            assert_eq!(mapped.error().code, expected, "http_code={http_code}");
        }
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
            Arc::new(ScriptedHttpGateway {
                final_url: None,
                error: None,
                captured: Arc::new(Mutex::new(None)),
            }),
        )
        .start()
        .await
        .unwrap();
        submit_and_wait(&runtime, snapshot.final_url).await;
        let CapturedSend { message, png } = wait_for_capture(&captured).await;
        let decoded = image::load_from_memory(&png).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (720, 466));
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

    #[tokio::test]
    async fn foreign_final_url_after_redirect_is_rejected_as_redirect_not_allowed() {
        let root = tempdir().unwrap();
        let snapshot = test_snapshot();
        let captured = Arc::new(Mutex::new(None));
        let http_capture = Arc::new(Mutex::new(None));
        let runtime = closed_loop_builder(
            test_service_config(root.path()),
            snapshot.clone(),
            captured,
            Arc::new(ScriptedHttpGateway {
                final_url: Some("https://evil.example/steal".into()),
                error: None,
                captured: http_capture.clone(),
            }),
        )
        .start()
        .await
        .unwrap();
        let waited = submit_and_wait_outcome(&runtime, snapshot.final_url).await;
        assert_eq!(waited.outcomes[0].status, "failed");
        assert_eq!(
            waited.outcomes[0].error_code.as_deref(),
            Some(ERROR_REDIRECT_NOT_ALLOWED)
        );
        assert_eq!(
            http_capture
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|request| request.limits.domain_allowlist.as_deref()),
            Some([MIHUASHI_DOMAIN.to_owned()].as_slice())
        );
        runtime.shutdown().await;
    }

    #[tokio::test]
    async fn oversized_http_body_maps_to_media_oversized() {
        assert_eq!(
            resolve_with_http_error(HttpGatewayError {
                code: HttpErrorCode::BodyTooLarge,
                message: "streamed response exceeded the configured response limit".into(),
                evidence: std::collections::BTreeMap::from([(
                    "observed_bytes".into(),
                    "12".into(),
                )]),
            })
            .await,
            ERROR_MEDIA_OVERSIZED
        );
    }

    #[tokio::test]
    async fn media_timeout_http_error_maps_to_media_timeout() {
        assert_eq!(
            resolve_with_http_error(HttpGatewayError {
                code: HttpErrorCode::TotalTimeout,
                message: "request exceeded the configured total timeout".into(),
                evidence: Default::default(),
            })
            .await,
            ERROR_MEDIA_TIMEOUT
        );
    }

    async fn resolve_with_http_error(error: HttpGatewayError) -> String {
        let root = tempdir().unwrap();
        let snapshot = test_snapshot();
        let captured = Arc::new(Mutex::new(None));
        let runtime = closed_loop_builder(
            test_service_config(root.path()),
            snapshot.clone(),
            captured,
            Arc::new(ScriptedHttpGateway {
                final_url: Some("https://img.mihuashi.com/a.jpg".into()),
                error: Some(error),
                captured: Arc::new(Mutex::new(None)),
            }),
        )
        .start()
        .await
        .unwrap();
        let waited = submit_and_wait_outcome(&runtime, snapshot.final_url).await;
        assert_eq!(waited.outcomes[0].status, "failed");
        let code = waited.outcomes[0].error_code.clone().expect("error code");
        runtime.shutdown().await;
        code
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
        http_gateway: Arc<dyn HttpGateway>,
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
        image_manifest.requires.push(SurfaceRequirement::new(
            ContractSurfaceKind::ResourceProvider,
            mutsuki_plugin_resource_memory::PLUGIN_ID,
        ));
        let mut mihuashi_manifest = manifest();
        mihuashi_manifest.requires.extend([
            SurfaceRequirement::task_protocol(SNAPSHOT),
            SurfaceRequirement::new(
                ContractSurfaceKind::ResourceProvider,
                mutsuki_plugin_resource_memory::PLUGIN_ID,
            ),
        ]);
        let mut http_manifest = mutsuki_plugin_io_http_client::manifest();
        http_manifest.requires.push(SurfaceRequirement::new(
            ContractSurfaceKind::ResourceProvider,
            mutsuki_plugin_resource_memory::PLUGIN_ID,
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
                        http_gateway.clone(),
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
        let waited = submit_and_wait_outcome(runtime, url).await;
        assert!(!waited.timed_out);
        assert_eq!(waited.outcomes[0].status, "completed");
    }

    async fn submit_and_wait_outcome(runtime: &ServiceRuntime, url: String) -> TaskWaitResponse {
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
        assert!(
            matches!(
                submit,
                ControlResponse::Ok(ControlResult::TaskSubmitBatch(_))
            ),
            "submit failed: {submit:?}"
        );
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
        waited
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
