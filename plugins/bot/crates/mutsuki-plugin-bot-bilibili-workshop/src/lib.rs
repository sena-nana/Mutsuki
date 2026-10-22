// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::default_trait_access,
    clippy::missing_panics_doc,
    clippy::needless_pass_by_value
)]

//! Bilibili Workshop (`mall.bilibili.com`) link resolver.
//!
//! The runner never opens a socket: the item page and the cover image are both fetched
//! through the std HTTP effect (`mutsuki.http.request@2`), which owns redirect, SSRF,
//! allowlist and size policy. The cover body resource the effect stores is sent as-is.

use std::sync::Arc;

use mutsuki_bot_link_parser::{MAX_LINK_CARD_MEDIA_BYTES, ResolvedLinkCard, preferred_event_url};
use mutsuki_bot_protocol::{
    BOT_MESSAGE_SEND_PROTOCOL_ID, BotEvent, BotExtMap, BotFlowEventEnvelope, BotFlowTypeRef,
    BotMessage, BotNodeBinding, BotNodeCatalogFragment, BotNodeDescriptor, BotNodeInvocation,
    BotNodeOutput, BotNodePortDescriptor, BotNodePortDirection, BotNodeResult, BotNodeRole,
    BotTarget, MessageSegment,
};
use mutsuki_protocol_http::{HttpRequest, HttpResponse, REQUEST as HTTP_REQUEST};
use mutsuki_runtime_contracts::{
    ExecutionClass, ProtocolClass, ReadPlan, ResourceRef, RunnerDescriptor, RunnerPurity,
    RunnerResult, RuntimeError, ScalarValue, Task, TaskOutcome,
};
use mutsuki_runtime_sdk::{
    AsyncRunnerContext, PluginBuilder, ProtocolDescriptorBuilder, ResourceRegistryGateway,
    RunnerDescriptorBuilder, RuntimeClientRef, TaskAwaitRunnerAdapter,
};
use mutsuki_runtime_sdk::{Runner, RuntimeFailure, RuntimeResult};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

pub const PLUGIN_ID: &str = "mutsuki.plugin.bot.bilibili-workshop";
pub const RUNNER_ID: &str = "mutsuki.bot.bilibili.workshop.runner";
pub const LINK_RESOLVE: &str = "mutsuki.bot.bilibili.workshop.link/resolve@1";

/// Hosts Workshop fetches from; the product grants these to the HTTP client.
pub const WORKSHOP_DOMAINS: [&str; 2] = ["mall.bilibili.com", "hdslb.com"];
const USER_AGENT: &str = "MutsukiBot/0.1 BilibiliWorkshop";
const MAX_PAGE_BYTES: u64 = 2 * 1024 * 1024;
const CONNECT_TIMEOUT_MS: u64 = 5_000;
const HEADER_TIMEOUT_MS: u64 = 10_000;
const IDLE_TIMEOUT_MS: u64 = 10_000;
const TOTAL_TIMEOUT_MS: u64 = 15_000;
const MAX_REDIRECTS: u8 = 5;

const ERROR_RESOLVE_FAILED: &str = "bilibili.workshop.resolve_failed";
const ERROR_URL_NOT_ALLOWED: &str = "bilibili.workshop.url_not_allowed";
const ERROR_REDIRECT_NOT_ALLOWED: &str = "bilibili.workshop.redirect_not_allowed";
const ERROR_RESPONSE_OVERSIZED: &str = "bilibili.workshop.response_oversized";
const ERROR_FETCH_TIMEOUT: &str = "bilibili.workshop.fetch_timeout";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkshopResolveRequest {
    pub url: String,
    pub target: BotTarget,
    pub outbound_binding: String,
}

/// Builds the Workshop runner. Both HTTP fetches are child tasks of the resolve task, and
/// the HTML body is read back through `resources`.
pub fn runner(
    client: RuntimeClientRef,
    resources: Arc<dyn ResourceRegistryGateway>,
) -> Box<dyn Runner> {
    let factory = Box::new(move |ctx: AsyncRunnerContext, task: Task| {
        let resources = resources.clone();
        Box::pin(async move { run_task(ctx, task, resources).await })
            as std::pin::Pin<
                Box<dyn std::future::Future<Output = RuntimeResult<RunnerResult>> + Send>,
            >
    });
    Box::new(
        TaskAwaitRunnerAdapter::new(descriptor(), client, factory).with_self_call_policy(false),
    )
}

async fn run_task(
    ctx: AsyncRunnerContext,
    task: Task,
    resources: Arc<dyn ResourceRegistryGateway>,
) -> RuntimeResult<RunnerResult> {
    let payload: Value = task.payload.clone().into();
    let invocation = <BotNodeInvocation as serde::Deserialize>::deserialize(&payload).ok();
    let request = match &invocation {
        Some(invocation) => {
            workshop_request_from_invocation(invocation).map_err(|error| fail(&task, error))?
        }
        None => serde_json::from_value(payload).map_err(|error| fail(&task, error))?,
    };
    let page = fetch(&ctx, &task, &request.url, MAX_PAGE_BYTES).await?;
    let html = resources.collect_read_plan(&ReadPlan {
        plan_id: format!("bilibili.workshop.page.read.{}", task.task_id),
        resource: page.body,
        operation: "collect".into(),
        args: Value::Null,
    })?;
    let card = parse_workshop_page(&String::from_utf8_lossy(&html), &page.final_url)
        .map_err(|error| fail(&task, error))?;
    let mut segments = Vec::new();
    if let Some(image_url) = card.image_url {
        let cover = fetch(&ctx, &task, &image_url, MAX_LINK_CARD_MEDIA_BYTES as u64).await?;
        segments.push(MessageSegment::Image {
            resource: cover.body,
        });
    }
    segments.push(MessageSegment::Text {
        text: format!("{}\n{}\n{}", card.title, card.description, card.url),
    });
    let outbound_binding = request.outbound_binding.clone();
    let message = BotMessage {
        message_id: None,
        target: request.target,
        sender: None,
        segments,
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

struct FetchedBody {
    final_url: String,
    body: ResourceRef,
}

/// One allowlisted GET through the HTTP effect. The URL is checked before the child task
/// is submitted and the final URL again afterwards, so a redirect the effect followed
/// cannot leave the Workshop domains even if the effect's own allowlist were wider.
async fn fetch(
    ctx: &AsyncRunnerContext,
    task: &Task,
    url: &str,
    max_bytes: u64,
) -> RuntimeResult<FetchedBody> {
    allow_workshop_url(url).map_err(|error| fail_code(task, ERROR_URL_NOT_ALLOWED, error))?;
    let outcome = ctx
        .call_raw(
            HTTP_REQUEST,
            serde_json::to_value(workshop_http_request(url, max_bytes))
                .map_err(|error| fail(task, error))?,
        )
        .await?;
    let response = decode_http_response(task, outcome)?;
    if !(200..300).contains(&response.metadata.status) {
        return Err(fail(
            task,
            format!("{url} returned HTTP status {}", response.metadata.status),
        ));
    }
    allow_workshop_url(&response.metadata.final_url)
        .map_err(|error| fail_code(task, ERROR_REDIRECT_NOT_ALLOWED, error))?;
    let body = response
        .body
        .ok_or_else(|| fail(task, format!("{url} returned an empty body")))?;
    Ok(FetchedBody {
        final_url: response.metadata.final_url,
        body,
    })
}

fn workshop_http_request(url: &str, max_bytes: u64) -> HttpRequest {
    let mut request = HttpRequest::get(url);
    request
        .headers
        .insert("User-Agent".into(), USER_AGENT.into());
    request.limits.max_response_bytes = Some(max_bytes);
    request.limits.connect_timeout_ms = Some(CONNECT_TIMEOUT_MS);
    request.limits.header_timeout_ms = Some(HEADER_TIMEOUT_MS);
    request.limits.idle_timeout_ms = Some(IDLE_TIMEOUT_MS);
    request.limits.total_timeout_ms = Some(TOTAL_TIMEOUT_MS);
    request.limits.max_redirects = Some(MAX_REDIRECTS);
    request.limits.domain_allowlist = Some(
        WORKSHOP_DOMAINS
            .iter()
            .map(|&domain| domain.into())
            .collect(),
    );
    request
}

fn decode_http_response(
    task: &Task,
    outcome: impl Into<TaskOutcome>,
) -> RuntimeResult<HttpResponse> {
    match outcome.into() {
        TaskOutcome::Completed {
            output: Some(output),
            ..
        } => serde_json::from_value(output).map_err(|error| fail(task, error)),
        TaskOutcome::Completed { output: None, .. } => {
            Err(fail(task, "HTTP request completed without output"))
        }
        TaskOutcome::Failed { error, .. } => Err(map_http_failure(task, error)),
        TaskOutcome::Cancelled { .. } | TaskOutcome::Expired { .. } => Err(fail_code(
            task,
            ERROR_FETCH_TIMEOUT,
            "HTTP request timed out",
        )),
        TaskOutcome::DeadLetter { .. } => Err(fail(task, "HTTP request was dead-lettered")),
    }
}

fn map_http_failure(task: &Task, error: RuntimeError) -> RuntimeFailure {
    let code = match error.code.as_str() {
        "http.domain_denied" | "http.https_required" | "http.invalid_url" => ERROR_URL_NOT_ALLOWED,
        "http.redirect_denied" | "http.too_many_redirects" | "http.private_address" => {
            ERROR_REDIRECT_NOT_ALLOWED
        }
        "http.body_too_large" => ERROR_RESPONSE_OVERSIZED,
        "http.header_timeout" | "http.idle_timeout" | "http.total_timeout" => ERROR_FETCH_TIMEOUT,
        _ => ERROR_RESOLVE_FAILED,
    };
    let mut mapped = RuntimeError::new(code, PLUGIN_ID, format!("workshop.{}", task.task_id));
    mapped.evidence = error.evidence;
    mapped
        .evidence
        .insert("http_code".into(), ScalarValue::String(error.code.clone()));
    mapped
        .evidence
        .insert("detail".into(), ScalarValue::String(error.route.clone()));
    RuntimeFailure::new(mapped)
}

/// Reads the Open Graph card from the item page. A relative `og:image` resolves against
/// the page URL; the result is re-checked by the allowlist before it is fetched.
fn parse_workshop_page(html: &str, final_url: &str) -> Result<ResolvedLinkCard, String> {
    let document = Html::parse_document(html);
    let meta = |property: &str| -> Option<String> {
        let selector = Selector::parse(&format!("meta[property='{property}']")).ok()?;
        document
            .select(&selector)
            .next()?
            .value()
            .attr("content")
            .map(ToOwned::to_owned)
    };
    let page_url = Url::parse(final_url).map_err(|error| error.to_string())?;
    let image_url = meta("og:image")
        .filter(|value| !value.is_empty())
        .map(|value| page_url.join(&value).map(String::from))
        .transpose()
        .map_err(|error| error.to_string())?;
    Ok(ResolvedLinkCard {
        url: final_url.to_owned(),
        title: meta("og:title").ok_or("workshop title is missing")?,
        description: meta("og:description").unwrap_or_default(),
        image_url,
    })
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
            workshop_node_catalog()
                .into_plugin_extension()
                .expect("workshop node catalog serializes"),
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
        .requires_protocol(HTTP_REQUEST)
        .purity(RunnerPurity::Effectful)
        .execution_class(ExecutionClass::Orchestration)
        .metadata("domain", ScalarValue::String("bilibili_workshop".into()))
        .build()
}

fn workshop_node_catalog() -> BotNodeCatalogFragment {
    BotNodeCatalogFragment {
        nodes: vec![BotNodeDescriptor {
            node_type_id: "mutsuki.bot.bilibili.workshop.resolve".into(),
            version: 1,
            title: "工房链接".into(),
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
                    event_type: BotFlowTypeRef::new(
                        mutsuki_bot_protocol::BOT_FLOW_MESSAGE_SEND_TYPE,
                        1,
                    ),
                    required: false,
                },
            ],
            config_schema: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "url": {"type": "string", "title": "工房链接"}
                }
            }),
        }],
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct WorkshopFlowConfig {
    url: Option<String>,
    outbound_binding: Option<String>,
}

fn workshop_request_from_invocation(
    invocation: &BotNodeInvocation,
) -> Result<WorkshopResolveRequest, String> {
    let config: WorkshopFlowConfig =
        serde::Deserialize::deserialize(&invocation.config).map_err(|error| error.to_string())?;
    let event: BotEvent = serde::Deserialize::deserialize(&invocation.input.payload.value)
        .map_err(|error| error.to_string())?;
    let url = config
        .url
        .filter(|value| !value.is_empty())
        .or_else(|| preferred_event_url(&event, &WORKSHOP_DOMAINS))
        .ok_or_else(|| "workshop url is missing".to_string())?;
    Ok(WorkshopResolveRequest {
        url,
        target: event.target,
        outbound_binding: config.outbound_binding.unwrap_or_default(),
    })
}

fn flow_message_result(
    task: &Task,
    invocation: BotNodeInvocation,
    message: BotMessage,
) -> RuntimeResult<RunnerResult> {
    let output = BotNodeOutput {
        port_id: "message".into(),
        event: BotFlowEventEnvelope::outbound_message(
            &invocation.input,
            invocation.input.event_id.clone(),
            serde_json::to_value(message).map_err(|error| fail(task, error))?,
        ),
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

fn allow_workshop_url(value: &str) -> Result<(), String> {
    let url = Url::parse(value).map_err(|error| format!("workshop url invalid: {error}"))?;
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && (host == "mall.bilibili.com"
            || host.ends_with(".mall.bilibili.com")
            || host == "hdslb.com"
            || host.ends_with(".hdslb.com"))
    {
        Ok(())
    } else {
        Err(format!("workshop domain denied: {host}"))
    }
}

fn fail(task: &Task, detail: impl std::fmt::Display) -> RuntimeFailure {
    fail_code(task, ERROR_RESOLVE_FAILED, detail)
}

fn fail_code(task: &Task, code: &str, detail: impl std::fmt::Display) -> RuntimeFailure {
    let mut error = RuntimeError::new(code, PLUGIN_ID, format!("workshop.{}", task.task_id));
    error
        .evidence
        .insert("detail".into(), ScalarValue::String(detail.to_string()));
    RuntimeFailure::new(error)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Duration;

    use async_trait::async_trait;
    use mutsuki_plugin_io_http_client::{
        FetchedHttpResponse, HttpEffectHandler, HttpGateway, HttpGatewayError,
    };
    use mutsuki_protocol_http::{HttpErrorCode, HttpResponseMetadata, RESPONSE_BODY_SCHEMA};
    use mutsuki_runtime_contracts::{
        ContractSurfaceKind, PluginManifest, SurfaceRequirement, TaskBatch, WorkBatch,
    };
    use mutsuki_runtime_sdk::map_work_batch_entries;
    use mutsuki_service_config::{ConfiguredPluginSelection, ServiceConfig};
    use mutsuki_service_control::{
        ControlCommand, ControlRequest, ControlResponse, ControlResult, TaskSubmitBatchParam,
        TaskWaitParam,
    };
    use mutsuki_service_runtime::ServiceRuntimeBuilder;
    use tempfile::tempdir;

    use super::*;

    const ITEM_URL: &str =
        "https://mall.bilibili.com/neul-next/detailuniversal/detail.html?itemsId=1";
    const COVER_URL: &str = "https://i0.hdslb.com/bfs/mall/cover.png";
    const COVER_PNG: &[u8] = b"\x89PNG\r\n\x1a\nworkshop-cover";
    const CAPTURE_PLUGIN_ID: &str = "workshop.test.bot-send";

    /// Serves the item page for [`ITEM_URL`] and the cover bytes for [`COVER_URL`].
    struct ScriptedHttpGateway {
        requests: Arc<Mutex<Vec<HttpRequest>>>,
    }

    #[async_trait]
    impl HttpGateway for ScriptedHttpGateway {
        async fn execute(
            &self,
            request: HttpRequest,
            _request_body: Option<Vec<u8>>,
        ) -> Result<FetchedHttpResponse, HttpGatewayError> {
            self.requests.lock().unwrap().push(request.clone());
            let body = match request.url.as_str() {
                ITEM_URL => format!(
                    "<html><head><meta property='og:title' content='工房商品'>\
                     <meta property='og:description' content='限定周边'>\
                     <meta property='og:image' content='{COVER_URL}'></head></html>"
                )
                .into_bytes(),
                COVER_URL => COVER_PNG.to_vec(),
                other => {
                    return Err(HttpGatewayError {
                        code: HttpErrorCode::DomainDenied,
                        message: format!("unscripted url {other}"),
                        evidence: Default::default(),
                    });
                }
            };
            Ok(FetchedHttpResponse {
                metadata: HttpResponseMetadata {
                    status: 200,
                    final_url: request.url.clone(),
                    headers: Default::default(),
                    body_bytes: body.len() as u64,
                    redirects_followed: 0,
                },
                peak_buffered_bytes: body.len() as u64,
                body,
            })
        }
    }

    struct CapturedSend {
        message: BotMessage,
        cover: Vec<u8>,
    }

    type CapturedSendSlot = Arc<Mutex<Option<CapturedSend>>>;

    struct CaptureSendRunner {
        descriptor: RunnerDescriptor,
        resources: Arc<dyn ResourceRegistryGateway>,
        captured: CapturedSendSlot,
    }

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
                let capture_error = |detail: String| {
                    RuntimeError::new("workshop.capture_failed", CAPTURE_PLUGIN_ID, detail)
                };
                let message: BotMessage = serde_json::from_value(task.payload.to_value())
                    .map_err(|error| capture_error(error.to_string()))?;
                let Some(MessageSegment::Image { resource }) = message.segments.first() else {
                    return Err(capture_error("image segment missing".into()));
                };
                assert_eq!(resource.schema, RESPONSE_BODY_SCHEMA);
                let cover = resources
                    .collect_read_plan(&ReadPlan {
                        plan_id: format!("workshop.capture.{}", task.task_id),
                        resource: resource.clone(),
                        operation: "collect".into(),
                        args: Value::Null,
                    })
                    .map_err(|error| capture_error(error.to_string()))?;
                *captured.lock().unwrap() = Some(CapturedSend { message, cover });
                Ok(RunnerResult::completed(task.task_id.clone()))
            })
        }
    }

    fn capture_descriptor() -> RunnerDescriptor {
        RunnerDescriptorBuilder::new("workshop.test.bot-send.runner", CAPTURE_PLUGIN_ID)
            .accepted_protocol(BOT_MESSAGE_SEND_PROTOCOL_ID)
            .purity(RunnerPurity::Effectful)
            .execution_class(ExecutionClass::Io)
            .build()
    }

    fn capture_manifest() -> PluginManifest {
        let mut manifest = PluginBuilder::new(CAPTURE_PLUGIN_ID)
            .runner_descriptor(capture_descriptor())
            .protocol_handler(
                ProtocolDescriptorBuilder::new(BOT_MESSAGE_SEND_PROTOCOL_ID)
                    .input_schema(json!({"type": "object"}))
                    .output_schema(json!({"type": "object"}))
                    .error_schema(json!({"type": "object"}))
                    .build(),
                "workshop.test.bot-send.runner",
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
    fn manifest_and_domain_policy() {
        let manifest = manifest();
        assert_eq!(
            manifest.provides.protocol_classes.get(LINK_RESOLVE),
            Some(&ProtocolClass::Effect)
        );
        assert!(
            manifest
                .requires
                .contains(&SurfaceRequirement::task_protocol(HTTP_REQUEST))
        );
        assert!(allow_workshop_url("https://evil.example/item").is_err());
        assert!(allow_workshop_url("https://www.bilibili.com/video/BV1").is_err());
        assert!(allow_workshop_url("http://mall.bilibili.com/item").is_err());
        assert!(allow_workshop_url("https://user:pass@mall.bilibili.com/item").is_err());
        assert!(allow_workshop_url("https://mall.bilibili.com/item").is_ok());
        assert!(allow_workshop_url("https://i0.hdslb.com/bfs/a.png").is_ok());
    }

    #[test]
    fn workshop_http_request_pins_allowlist_budgets_and_deadlines() {
        let request = workshop_http_request(ITEM_URL, MAX_PAGE_BYTES);
        assert_eq!(request.url, ITEM_URL);
        assert_eq!(
            request.headers.get("User-Agent").map(String::as_str),
            Some(USER_AGENT)
        );
        assert!(request.response_body.is_resource());
        assert_eq!(request.limits.max_response_bytes, Some(MAX_PAGE_BYTES));
        assert_eq!(request.limits.connect_timeout_ms, Some(CONNECT_TIMEOUT_MS));
        assert_eq!(request.limits.header_timeout_ms, Some(HEADER_TIMEOUT_MS));
        assert_eq!(request.limits.idle_timeout_ms, Some(IDLE_TIMEOUT_MS));
        assert_eq!(request.limits.total_timeout_ms, Some(TOTAL_TIMEOUT_MS));
        assert_eq!(request.limits.max_redirects, Some(MAX_REDIRECTS));
        assert_eq!(
            request.limits.domain_allowlist,
            Some(vec!["mall.bilibili.com".into(), "hdslb.com".into()])
        );
    }

    #[test]
    fn http_failures_map_to_distinct_workshop_codes() {
        let task = Task::new("t1", LINK_RESOLVE, json!({}));
        for (http_code, expected) in [
            ("http.domain_denied", ERROR_URL_NOT_ALLOWED),
            ("http.redirect_denied", ERROR_REDIRECT_NOT_ALLOWED),
            ("http.private_address", ERROR_REDIRECT_NOT_ALLOWED),
            ("http.body_too_large", ERROR_RESPONSE_OVERSIZED),
            ("http.total_timeout", ERROR_FETCH_TIMEOUT),
            ("http.connect_failed", ERROR_RESOLVE_FAILED),
        ] {
            let error = RuntimeError::new(http_code, "runtime.io_http_client", "boom");
            assert_eq!(
                map_http_failure(&task, error).error().code,
                expected,
                "http_code={http_code}"
            );
        }
    }

    #[test]
    fn page_parser_resolves_protocol_relative_cover() {
        let card = parse_workshop_page(
            "<meta property='og:title' content='T'><meta property='og:image' content='//i0.hdslb.com/a.png'>",
            ITEM_URL,
        )
        .unwrap();
        assert_eq!(card.title, "T");
        assert_eq!(
            card.image_url.as_deref(),
            Some("https://i0.hdslb.com/a.png")
        );
    }

    #[tokio::test]
    async fn real_core_fetches_page_and_cover_through_http_effect() {
        let root = tempdir().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured: CapturedSendSlot = Arc::new(Mutex::new(None));
        let runtime = closed_loop_builder(
            test_service_config(root.path()),
            Arc::new(ScriptedHttpGateway {
                requests: requests.clone(),
            }),
            captured.clone(),
        )
        .start()
        .await
        .unwrap();

        let control = runtime.control_handler();
        let request = WorkshopResolveRequest {
            url: ITEM_URL.into(),
            target: BotTarget::Group {
                group_id: "group-1".into(),
            },
            outbound_binding: "qq-main".into(),
        };
        let submit = control
            .handle(ControlRequest::new(
                runtime.control_token(),
                ControlCommand::TaskSubmitBatch(TaskSubmitBatchParam {
                    batch: TaskBatch::one(
                        "workshop-real-core-batch",
                        Task::new(
                            "workshop-real-core",
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
                    ids: vec!["workshop-real-core".into()],
                    timeout_ms: 10_000,
                }),
            ))
            .await;
        let ControlResponse::Ok(ControlResult::TaskWait(waited)) = waited else {
            panic!("wait failed: {waited:?}");
        };
        assert!(!waited.timed_out);
        assert_eq!(
            waited.outcomes[0].status, "completed",
            "{:?}",
            waited.outcomes[0]
        );

        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while captured.lock().unwrap().is_none() {
            assert!(tokio::time::Instant::now() < deadline, "Bot send timed out");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let CapturedSend { message, cover } = captured.lock().unwrap().take().unwrap();
        assert_eq!(cover, COVER_PNG);
        assert_eq!(
            message.segments[1],
            MessageSegment::Text {
                text: format!("工房商品\n限定周边\n{ITEM_URL}")
            }
        );
        let requests = requests.lock().unwrap().clone();
        assert_eq!(
            requests
                .iter()
                .map(|request| request.url.as_str())
                .collect::<Vec<_>>(),
            [ITEM_URL, COVER_URL]
        );
        assert_eq!(
            requests[1].limits.max_response_bytes,
            Some(MAX_LINK_CARD_MEDIA_BYTES as u64)
        );
        runtime.shutdown().await;
    }

    fn test_service_config(root: &std::path::Path) -> ServiceConfig {
        std::fs::create_dir_all(root.join("logs")).unwrap();
        let mut config = ServiceConfig::default();
        config.ipc.enabled = false;
        config.ipc.token = Some("workshop-test-token".into());
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
            mutsuki_plugin_io_http_client::PLUGIN_ID,
            PLUGIN_ID,
            CAPTURE_PLUGIN_ID,
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
        http_gateway: Arc<dyn HttpGateway>,
        captured: CapturedSendSlot,
    ) -> ServiceRuntimeBuilder {
        let memory_provider = || {
            SurfaceRequirement::new(
                ContractSurfaceKind::ResourceProvider,
                mutsuki_plugin_resource_memory::PLUGIN_ID,
            )
        };
        let mut http_manifest = mutsuki_plugin_io_http_client::manifest();
        http_manifest.requires.push(memory_provider());
        let mut workshop_manifest = manifest();
        workshop_manifest.requires.push(memory_provider());
        ServiceRuntimeBuilder::new(config)
            .register_builtin_loaded_plugin_factory(
                mutsuki_plugin_resource_memory::loaded_plugin().manifest,
                || Ok::<_, String>(mutsuki_plugin_resource_memory::loaded_plugin()),
            )
            .register_builtin_plugin(http_manifest)
            .register_runtime_client_runner(mutsuki_plugin_io_http_client::facade_runner)
            .register_fallible_runtime_services_async_handler(move |_client, resources| {
                Ok::<Arc<dyn mutsuki_runtime_sdk::AsyncBatchHandler>, String>(Arc::new(
                    HttpEffectHandler::new(
                        http_gateway.clone(),
                        resources,
                        mutsuki_plugin_resource_memory::PLUGIN_ID,
                    ),
                ))
            })
            .register_builtin_plugin(workshop_manifest)
            .register_runtime_services_runner(runner)
            .register_builtin_plugin(capture_manifest())
            .register_runtime_services_runner(move |_client, resources| {
                Box::new(CaptureSendRunner {
                    descriptor: capture_descriptor(),
                    resources,
                    captured: captured.clone(),
                })
            })
    }
}
