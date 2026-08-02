use std::collections::BTreeMap;
use std::sync::Arc;

use futures_util::StreamExt;
use mutsuki_bot_protocol::{BOT_EVENT_INGEST_PROTOCOL_ID, BotMediaKind, MessageSegment};
use mutsuki_runtime_contracts::{
    CompletionBatch, EntryCompletion, ExecutionClass, InvocationMode, RunnerBatchCapability,
    RunnerConcurrency, RunnerMode, RunnerResult, RunnerSideEffect, RuntimeError, Task, WorkBatch,
};
use mutsuki_runtime_core::{AsyncBatchHandler, AsyncCompletionFuture, RunnerContext};
use mutsuki_runtime_sdk::{ResourceRegistryGateway, RunnerDescriptorBuilder};
use reqwest::{Client, Url};
use serde_json::Value;

use crate::adapter::qq_gateway_frame_to_bot_event;
use crate::tasks::{QQBOT_ADAPTER_PLUGIN_ID, QQBOT_GATEWAY_RUNNER_ID};
use crate::{GatewayFrame, QQBOT_GATEWAY_FRAME_PROTOCOL_ID, QqBotConfig};

pub struct QqGatewayMediaHandler {
    descriptor: mutsuki_runtime_contracts::RunnerDescriptor,
    account_id: String,
    provider_id: String,
    allow_insecure_transport: bool,
    allowed_hosts: Vec<String>,
    client: Client,
    resources: Arc<dyn ResourceRegistryGateway>,
    max_bytes_by_kind: BTreeMap<BotMediaKind, u64>,
}

impl QqGatewayMediaHandler {
    pub fn new(
        config: QqBotConfig,
        resources: Arc<dyn ResourceRegistryGateway>,
    ) -> Result<Self, String> {
        let provider_id = config
            .media_provider_id
            .clone()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "QQ inbound media requires media_provider_id".to_string())?;
        let client = Client::builder()
            .connect_timeout(std::time::Duration::from_millis(config.connect_timeout_ms))
            .timeout(std::time::Duration::from_millis(config.request_timeout_ms))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            descriptor: gateway_media_descriptor(1),
            account_id: config.account_id.clone(),
            provider_id,
            allow_insecure_transport: config.allow_insecure_transport,
            allowed_hosts: config
                .media_download_allowed_hosts
                .iter()
                .map(|host| host.to_ascii_lowercase())
                .collect(),
            client,
            resources,
            max_bytes_by_kind: config.capability_matrix().upload.max_bytes_by_kind,
        })
    }
}

impl AsyncBatchHandler for QqGatewayMediaHandler {
    fn descriptor(&self) -> &mutsuki_runtime_contracts::RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(&self, ctx: RunnerContext, batch: WorkBatch) -> AsyncCompletionFuture {
        let tasks = match batch.row_payload_tasks() {
            Ok(tasks) => tasks,
            Err(error) => {
                return Box::pin(async move { Ok(CompletionBatch::from_error(&batch, error)) });
            }
        };
        let account_id = self.account_id.clone();
        let provider_id = self.provider_id.clone();
        let allow_insecure_transport = self.allow_insecure_transport;
        let allowed_hosts = self.allowed_hosts.clone();
        let client = self.client.clone();
        let resources = self.resources.clone();
        let max_bytes_by_kind = self.max_bytes_by_kind.clone();
        Box::pin(async move {
            let mut results = Vec::with_capacity(batch.entries.len());
            for entry in &batch.entries {
                let task = tasks
                    .iter()
                    .find(|task| task.task_id == entry.task_id)
                    .expect("validated batch contains every task")
                    .clone();
                let completion = map_task_with_media(
                    &ctx,
                    task,
                    &account_id,
                    &provider_id,
                    allow_insecure_transport,
                    &allowed_hosts,
                    &client,
                    resources.as_ref(),
                    &max_bytes_by_kind,
                )
                .await;
                let (result, error) = match completion {
                    Ok(result) => (Some(result), None),
                    Err(error) => (None, Some(error)),
                };
                results.push(EntryCompletion {
                    entry_id: entry.entry_id.clone(),
                    task_id: entry.task_id.clone(),
                    result,
                    error,
                });
            }
            Ok(CompletionBatch::from_results(&batch, results))
        })
    }
}

pub fn gateway_media_descriptor(
    plugin_generation: u64,
) -> mutsuki_runtime_contracts::RunnerDescriptor {
    RunnerDescriptorBuilder::new(QQBOT_GATEWAY_RUNNER_ID, QQBOT_ADAPTER_PLUGIN_ID)
        .plugin_generation(plugin_generation)
        .accepted_protocol(QQBOT_GATEWAY_FRAME_PROTOCOL_ID)
        .execution_class(ExecutionClass::Io)
        .invocation_mode(InvocationMode::AsyncReentrant)
        .concurrency(RunnerConcurrency::Reentrant {
            max_inflight_batches: 16,
            max_inflight_entries: 64,
        })
        .batch_capability(RunnerBatchCapability {
            mode: RunnerMode::NativeBatch,
            preferred_batch_size: 8,
            max_batch_entries: 64,
            max_inflight_batches: 16,
            max_entry_concurrency: 64,
            side_effect: RunnerSideEffect::External,
            ..RunnerBatchCapability::default()
        })
        .build()
}

#[allow(clippy::too_many_arguments)]
async fn map_task_with_media(
    ctx: &RunnerContext,
    task: Task,
    account_id: &str,
    provider_id: &str,
    allow_insecure_transport: bool,
    allowed_hosts: &[String],
    client: &Client,
    resources: &dyn ResourceRegistryGateway,
    max_bytes_by_kind: &BTreeMap<BotMediaKind, u64>,
) -> Result<RunnerResult, RuntimeError> {
    let frame: GatewayFrame = serde_json::from_value(task.payload.clone().into())
        .map_err(|error| failure("gateway.decode", error))?;
    let attachments = attachments(&frame.d)?;
    let mut event = qq_gateway_frame_to_bot_event(account_id, frame)
        .map_err(|error| failure("gateway.map", error))?;
    if !attachments.is_empty() {
        let message = event
            .message
            .as_mut()
            .ok_or_else(|| failure("gateway.media.message_missing", "message missing"))?;
        for attachment in attachments {
            let max_bytes = max_bytes_by_kind
                .get(&attachment.kind)
                .copied()
                .ok_or_else(|| failure("gateway.media.unsupported", "unsupported media kind"))?;
            let bytes = download(
                client,
                &attachment.url,
                allow_insecure_transport,
                allowed_hosts,
                max_bytes,
                attachment.declared_size,
            )
            .await?;
            validate_mime(&attachment.mime_type, &bytes)?;
            let resource = resources
                .create_blob_resource(provider_id, &attachment.mime_type, bytes)
                .map_err(|error| failure("gateway.media.resource", error))?;
            if resource.size_hint.is_none()
                || resource.content_hash.as_deref().is_none_or(str::is_empty)
            {
                return Err(failure(
                    "gateway.media.resource_unvalidated",
                    "resource provider omitted size or content hash",
                ));
            }
            message.segments.push(match attachment.kind {
                BotMediaKind::Image => MessageSegment::Image { resource },
                BotMediaKind::Audio => MessageSegment::Audio { resource },
                BotMediaKind::Video => MessageSegment::Video { resource },
                BotMediaKind::File => MessageSegment::File {
                    resource,
                    name: attachment.file_name,
                },
            });
        }
    }
    let mut ingest = Task::new(
        format!("mutsuki.bot.event.ingest:{}", task.task_id),
        BOT_EVENT_INGEST_PROTOCOL_ID,
        mutsuki_runtime_contracts::TaskPayload::from_local(event),
    );
    ingest.registry_generation = ctx.registry_generation;
    ingest.trace_id = task.trace_id.clone();
    ingest.correlation_id = task.correlation_id.clone();
    let mut result = RunnerResult::completed(task.task_id);
    result.tasks.push(ingest);
    Ok(result)
}

struct QqAttachment {
    url: Url,
    mime_type: String,
    kind: BotMediaKind,
    file_name: Option<String>,
    declared_size: Option<u64>,
}

fn attachments(data: &Value) -> Result<Vec<QqAttachment>, RuntimeError> {
    let direct = data.get("attachments").and_then(Value::as_array);
    let referenced = data
        .get("referenced_message")
        .and_then(|message| message.get("attachments"))
        .and_then(Value::as_array);
    direct
        .into_iter()
        .chain(referenced)
        .flatten()
        .map(|value| {
            let url = value
                .get("url")
                .and_then(Value::as_str)
                .ok_or_else(|| failure("gateway.media.url_missing", "attachment URL missing"))?;
            let url = Url::parse(url).map_err(|error| failure("gateway.media.url", error))?;
            let mime_type = value
                .get("content_type")
                .or_else(|| value.get("contentType"))
                .and_then(Value::as_str)
                .unwrap_or("application/octet-stream")
                .to_ascii_lowercase();
            Ok(QqAttachment {
                url,
                kind: kind_for_mime(&mime_type),
                mime_type,
                file_name: value
                    .get("filename")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                declared_size: value
                    .get("size")
                    .and_then(|size| size.as_u64().or_else(|| size.as_str()?.parse().ok())),
            })
        })
        .collect()
}

fn kind_for_mime(mime_type: &str) -> BotMediaKind {
    if mime_type.starts_with("image/") {
        BotMediaKind::Image
    } else if mime_type.starts_with("audio/") {
        BotMediaKind::Audio
    } else if mime_type.starts_with("video/") {
        BotMediaKind::Video
    } else {
        BotMediaKind::File
    }
}

async fn download(
    client: &Client,
    url: &Url,
    allow_insecure_transport: bool,
    allowed_hosts: &[String],
    max_bytes: u64,
    declared_size: Option<u64>,
) -> Result<Vec<u8>, RuntimeError> {
    if url.username() != "" || url.password().is_some() || url.host_str().is_none() {
        return Err(failure(
            "gateway.media.url",
            "URL contains credentials or has no host",
        ));
    }
    if url.scheme() != "https" && !(allow_insecure_transport && url.scheme() == "http") {
        return Err(failure(
            "gateway.media.transport",
            "media URL must use HTTPS",
        ));
    }
    validate_download_host(url, allow_insecure_transport, allowed_hosts).await?;
    if declared_size.is_some_and(|size| size > max_bytes) {
        return Err(failure(
            "gateway.media.too_large",
            "declared size exceeds QQ limit",
        ));
    }
    let response = client
        .get(url.clone())
        .send()
        .await
        .map_err(|error| failure("gateway.media.download", error))?
        .error_for_status()
        .map_err(|error| failure("gateway.media.status", error))?;
    if response
        .content_length()
        .is_some_and(|size| size > max_bytes)
    {
        return Err(failure(
            "gateway.media.too_large",
            "response exceeds QQ limit",
        ));
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| failure("gateway.media.stream", error))?;
        if bytes.len().saturating_add(chunk.len()) as u64 > max_bytes {
            return Err(failure(
                "gateway.media.too_large",
                "stream exceeds QQ limit",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    if declared_size.is_some_and(|size| size != bytes.len() as u64) {
        return Err(failure(
            "gateway.media.size_mismatch",
            "download size differs from declaration",
        ));
    }
    Ok(bytes)
}

async fn validate_download_host(
    url: &Url,
    allow_insecure_transport: bool,
    allowed_hosts: &[String],
) -> Result<(), RuntimeError> {
    let host = url
        .host_str()
        .ok_or_else(|| failure("gateway.media.url", "media URL has no host"))?
        .to_ascii_lowercase();
    if allow_insecure_transport && url.scheme() == "http" {
        let ip = host.parse::<std::net::IpAddr>().map_err(|_| {
            failure(
                "gateway.media.test_host",
                "insecure test media URL must use a loopback IP literal",
            )
        })?;
        if !ip.is_loopback() {
            return Err(failure(
                "gateway.media.test_host",
                "insecure test media URL must use a loopback address",
            ));
        }
        return Ok(());
    }
    if !allowed_hosts
        .iter()
        .any(|allowed| host == *allowed || host.ends_with(&format!(".{allowed}")))
    {
        return Err(failure(
            "gateway.media.host_denied",
            "media URL host is not in the configured QQ attachment allowlist",
        ));
    }
    let port = url
        .port_or_known_default()
        .ok_or_else(|| failure("gateway.media.url", "media URL has no port"))?;
    let addresses = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|error| failure("gateway.media.dns", error))?
        .collect::<Vec<_>>();
    if addresses.is_empty() || addresses.iter().any(|address| unsafe_ip(address.ip())) {
        return Err(failure(
            "gateway.media.ssrf",
            "media URL resolves to a private or non-routable address",
        ));
    }
    Ok(())
}

fn unsafe_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => {
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip.octets()[0] == 0
        }
        std::net::IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || (ip.segments()[0] & 0xfe00) == 0xfc00
                || (ip.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

fn validate_mime(mime_type: &str, bytes: &[u8]) -> Result<(), RuntimeError> {
    let matches = match mime_type {
        "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
        "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "image/webp" => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"),
        "audio/wav" | "audio/x-wav" => {
            bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE")
        }
        "audio/ogg" | "application/ogg" => bytes.starts_with(b"OggS"),
        "audio/mpeg" | "audio/mp3" => {
            bytes.starts_with(b"ID3")
                || bytes
                    .get(..2)
                    .is_some_and(|header| header[0] == 0xff && header[1] & 0xe0 == 0xe0)
        }
        "video/mp4" => bytes.get(4..8) == Some(b"ftyp"),
        value
            if value.starts_with("image/")
                || value.starts_with("audio/")
                || value.starts_with("video/") =>
        {
            false
        }
        _ => true,
    };
    matches.then_some(()).ok_or_else(|| {
        failure(
            "gateway.media.mime_mismatch",
            "declared MIME does not match bytes",
        )
    })
}

fn failure(route: &str, error: impl std::fmt::Display) -> RuntimeError {
    let mut runtime_error = RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        QQBOT_ADAPTER_PLUGIN_ID,
        route,
    );
    runtime_error.evidence.insert(
        "reason".into(),
        mutsuki_runtime_contracts::ScalarValue::String(error.to_string()),
    );
    runtime_error
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use mutsuki_bot_protocol::{BotEvent, BotTarget};
    use mutsuki_runtime_contracts::{
        BatchEntry, BatchPayload, CommandPlan, DispatchLane, ExportPlan, OrderingRequirement,
        PlanReceipt, ReadPlan, ResourceAccess, ResourceId, ResourceLifetime, ResourceRef,
        ResourceSealState, ResourceSemantic, SnapshotDescriptor, StreamPlan, WorkResourcePlan,
        resource::experimental::{CommandBatch, SagaPlan},
    };
    use mutsuki_runtime_core::RuntimeFailure;
    use sha2::{Digest, Sha256};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    #[tokio::test]
    async fn private_group_and_channel_media_flow_through_fake_server_into_validated_resources() {
        for (event_type, target_fields, expected_target) in [
            (
                "C2C_MESSAGE_CREATE",
                serde_json::json!({"author": {"user_openid": "user"}}),
                BotTarget::User {
                    user_id: "user".into(),
                },
            ),
            (
                "GROUP_AT_MESSAGE_CREATE",
                serde_json::json!({"group_openid": "group", "author": {"member_openid": "actor"}}),
                BotTarget::Group {
                    group_id: "group".into(),
                },
            ),
            (
                "AT_MESSAGE_CREATE",
                serde_json::json!({"guild_id": "guild", "channel_id": "channel", "author": {"id": "actor"}}),
                BotTarget::GuildChannel {
                    guild_id: "guild".into(),
                    channel_id: "channel".into(),
                },
            ),
        ] {
            for (kind, mime_type, file_name, bytes) in [
                (BotMediaKind::Image, "image/png", "picture.png", PNG),
                (BotMediaKind::Audio, "audio/mpeg", "voice.mp3", MP3),
                (BotMediaKind::Video, "video/mp4", "clip.mp4", MP4),
                (BotMediaKind::File, "application/pdf", "document.pdf", PDF),
            ] {
                let (url, server) = media_server(bytes, mime_type).await;
                let resources = Arc::new(TestResources::default());
                let mut config = QqBotConfig::new("main", "app");
                config.allow_insecure_transport = true;
                config.media_provider_id = Some("memory".into());
                let handler = QqGatewayMediaHandler::new(config, resources.clone()).unwrap();
                let mut data = target_fields.as_object().unwrap().clone();
                data.insert("id".into(), Value::String(format!("{event_type}-message")));
                data.insert("content".into(), Value::String("look".into()));
                data.insert(
                    "attachments".into(),
                    serde_json::json!([{
                        "url": url,
                        "content_type": mime_type,
                        "filename": file_name,
                        "size": bytes.len()
                    }]),
                );
                let task = Task::new(
                    format!("task-{event_type}-{kind:?}"),
                    QQBOT_GATEWAY_FRAME_PROTOCOL_ID,
                    serde_json::json!({"op": 0, "s": 1, "t": event_type, "d": data}),
                );
                let result = handler
                    .run_batch(context(), batch(&task))
                    .await
                    .unwrap()
                    .results
                    .into_iter()
                    .next()
                    .unwrap()
                    .result
                    .unwrap();
                let event: BotEvent = result.tasks[0]
                    .payload
                    .decode_shared::<BotEvent>()
                    .unwrap()
                    .as_ref()
                    .clone();
                assert_eq!(event.target, expected_target.clone());
                let segment = event.message.unwrap().segments.pop().unwrap();
                let resource = match (kind, segment) {
                    (BotMediaKind::Image, MessageSegment::Image { resource })
                    | (BotMediaKind::Audio, MessageSegment::Audio { resource })
                    | (BotMediaKind::Video, MessageSegment::Video { resource })
                    | (BotMediaKind::File, MessageSegment::File { resource, .. }) => resource,
                    (_, segment) => panic!("unexpected media segment: {segment:?}"),
                };
                assert_eq!(resource.size_hint, Some(bytes.len() as u64));
                assert!(resource.content_hash.unwrap().starts_with("sha256:"));
                assert_eq!(resources.created.lock().unwrap().len(), 1);
                server.await.unwrap();
            }
        }
    }

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfixture";
    const MP3: &[u8] = b"ID3fixture";
    const MP4: &[u8] = b"\0\0\0\x18ftypfixture";
    const PDF: &[u8] = b"%PDF-1.7 fixture";

    #[test]
    fn mp3_id3_and_frame_sync_are_accepted_but_spoofed_audio_is_rejected() {
        assert!(validate_mime("audio/mpeg", b"ID3fixture").is_ok());
        assert!(validate_mime("audio/mpeg", &[0xff, 0xfb, 0x90, 0x64]).is_ok());
        assert!(validate_mime("audio/mpeg", b"not-an-mp3").is_err());
    }

    async fn media_server(
        bytes: &'static [u8],
        mime_type: &'static str,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = socket.read(&mut request).await.unwrap();
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {mime_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        bytes.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            socket.write_all(bytes).await.unwrap();
        });
        (format!("http://{address}/image"), server)
    }

    fn context() -> RunnerContext {
        RunnerContext::new(1, 1, "executor:test", None, "invocation:test").with_batch("batch", 1)
    }

    fn batch(task: &Task) -> WorkBatch {
        WorkBatch {
            batch_id: "batch".into(),
            tick_id: "tick".into(),
            batch_key: QQBOT_GATEWAY_RUNNER_ID.into(),
            entries: vec![BatchEntry {
                entry_id: "entry".into(),
                task_id: task.task_id.clone(),
                trace_id: task.trace_id.clone(),
                parent_id: None,
                payload_index: 0,
                resource_requirement_indices: Vec::new(),
                cancel_index: None,
                deadline_tick: None,
                priority: 0,
                lane: DispatchLane::Normal,
                ordering: OrderingRequirement::PreserveSubmitOrder,
            }],
            payload: BatchPayload::from_tasks(std::slice::from_ref(task)),
            resource_plan: WorkResourcePlan::empty(),
            task_leases: Vec::new(),
        }
    }

    #[derive(Default)]
    struct TestResources {
        created: Mutex<Vec<Vec<u8>>>,
    }

    impl mutsuki_runtime_sdk::ResourcePlanGateway for TestResources {
        fn collect_read_plan(&self, _: &ReadPlan) -> Result<Vec<u8>, RuntimeFailure> {
            Err(unused())
        }
        fn snapshot_read_plan(
            &self,
            _: &ReadPlan,
            _: &str,
            _: &str,
        ) -> Result<SnapshotDescriptor, RuntimeFailure> {
            Err(unused())
        }
        fn open_stream_plan(&self, _: &ReadPlan) -> Result<StreamPlan, RuntimeFailure> {
            Err(unused())
        }
        fn execute_export_plan(&self, _: &ExportPlan) -> Result<PlanReceipt, RuntimeFailure> {
            Err(unused())
        }
        fn commit_write_plan(
            &self,
            _: &mutsuki_runtime_contracts::WritePlan,
            _: Vec<u8>,
        ) -> Result<PlanReceipt, RuntimeFailure> {
            Err(unused())
        }
        fn execute_command_plan(&self, _: &CommandPlan) -> Result<PlanReceipt, RuntimeFailure> {
            Err(unused())
        }
        fn execute_command_batch(
            &self,
            _: &CommandBatch,
        ) -> Result<Vec<PlanReceipt>, RuntimeFailure> {
            Err(unused())
        }
        fn execute_saga_plan(&self, _: &SagaPlan) -> Result<Vec<PlanReceipt>, RuntimeFailure> {
            Err(unused())
        }
    }

    impl ResourceRegistryGateway for TestResources {
        fn open_resource_descriptor(&self, _: &str) -> Result<ResourceRef, RuntimeFailure> {
            Err(unused())
        }

        fn create_blob_resource(
            &self,
            provider_id: &str,
            schema: &str,
            bytes: Vec<u8>,
        ) -> Result<ResourceRef, RuntimeFailure> {
            let digest = format!("sha256:{:x}", Sha256::digest(&bytes));
            let size = bytes.len() as u64;
            self.created.lock().unwrap().push(bytes);
            Ok(ResourceRef {
                ref_id: format!("ref-{digest}"),
                resource_id: ResourceId {
                    kind_id: "blob".into(),
                    slot_id: digest.clone(),
                    generation: 1,
                    version: 1,
                },
                semantic: ResourceSemantic::FrozenValue,
                provider_id: provider_id.into(),
                resource_kind: "blob".into(),
                schema: schema.into(),
                version: 1,
                generation: 1,
                access: ResourceAccess::Inline,
                size_hint: Some(size),
                content_hash: Some(digest),
                lifetime: ResourceLifetime::LeaseUntil(100),
                lease: None,
                seal_state: ResourceSealState::Sealed,
            })
        }

        fn create_cow_state_resource(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: Vec<u8>,
        ) -> Result<ResourceRef, RuntimeFailure> {
            Err(unused())
        }

        fn create_capability_resource(
            &self,
            _: &str,
            _: &str,
            _: &str,
        ) -> Result<ResourceRef, RuntimeFailure> {
            Err(unused())
        }
    }

    fn unused() -> RuntimeFailure {
        RuntimeFailure::new(RuntimeError::new(
            mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
            "test",
            "unused",
        ))
    }
}
