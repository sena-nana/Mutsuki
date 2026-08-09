use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use mutsuki_bot_link_parser::{MAX_LINK_CARD_MEDIA_BYTES, ResolvedLinkCard};
use mutsuki_bot_protocol::{
    BOT_MESSAGE_SEND_PROTOCOL_ID, BotExtMap, BotMessage, BotTarget, MessageSegment,
};
use mutsuki_runtime_contracts::{
    CompletionBatch, ExecutionClass, ProtocolClass, RunnerContext, RunnerDescriptor, RunnerPurity,
    RunnerResult, RuntimeError, ScalarValue, Task, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RuntimeResult};
use mutsuki_runtime_sdk::{
    PluginBuilder, ProtocolDescriptorBuilder, ResourceRegistryGateway, RunnerDescriptorBuilder,
    map_work_batch_entries,
};
use reqwest::blocking::Client;
use reqwest::header::LOCATION;
use reqwest::redirect::Policy;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::json;
use url::Url;

pub const PLUGIN_ID: &str = "mutsuki.bot.bilibili.workshop";
pub const RUNNER_ID: &str = "mutsuki.bot.bilibili.workshop.runner";
pub const LINK_RESOLVE: &str = "mutsuki.bot.bilibili.workshop.link/resolve@1";

const MEDIA_MAX_REDIRECTS: u8 = 5;
const MEDIA_TIMEOUT: Duration = Duration::from_secs(15);
const READ_CHUNK: usize = 8 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkshopResolveRequest {
    pub url: String,
    pub target: BotTarget,
    pub outbound_binding: String,
}

pub trait WorkshopTransport: Send {
    fn resolve(&mut self, url: &str) -> Result<ResolvedLinkCard, String>;
    fn download(&mut self, url: &str, max_bytes: usize) -> Result<Vec<u8>, String>;
}

#[derive(Default)]
pub struct ReqwestWorkshopTransport {
    client: Option<Client>,
}
impl ReqwestWorkshopTransport {
    pub fn new() -> Self {
        Self::default()
    }
    fn client(&mut self) -> Result<&Client, String> {
        if self.client.is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
            self.client = Some(
                Client::builder()
                    .timeout(MEDIA_TIMEOUT)
                    .user_agent("MutsukiBot/0.1 BilibiliWorkshop")
                    .redirect(Policy::none())
                    .build()
                    .map_err(|error| error.to_string())?,
            );
        }
        Ok(self.client.as_ref().expect("client initialized"))
    }
}
impl WorkshopTransport for ReqwestWorkshopTransport {
    fn resolve(&mut self, url: &str) -> Result<ResolvedLinkCard, String> {
        let (final_url, bytes) = fetch_bytes(
            self.client()?,
            url,
            MAX_LINK_CARD_MEDIA_BYTES,
            allow_workshop_url,
        )?;
        let html = String::from_utf8(bytes).map_err(|error| error.to_string())?;
        let document = Html::parse_document(&html);
        let meta = |property: &str| -> Option<String> {
            let selector = Selector::parse(&format!("meta[property='{property}']")).ok()?;
            document
                .select(&selector)
                .next()?
                .value()
                .attr("content")
                .map(ToOwned::to_owned)
        };
        Ok(ResolvedLinkCard {
            url: final_url.to_string(),
            title: meta("og:title").ok_or("workshop title is missing")?,
            description: meta("og:description").unwrap_or_default(),
            image_url: meta("og:image"),
        })
    }
    fn download(&mut self, url: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
        fetch_bytes(self.client()?, url, max_bytes, allow_workshop_url).map(|(_, bytes)| bytes)
    }
}

fn fetch_bytes(
    client: &Client,
    url: &str,
    max_bytes: usize,
    allow: impl Fn(&Url) -> Result<(), String>,
) -> Result<(Url, Vec<u8>), String> {
    let mut current = Url::parse(url).map_err(|error| error.to_string())?;
    for hop in 0..=MEDIA_MAX_REDIRECTS {
        allow(&current)?;
        let mut response = client
            .get(current.as_str())
            .send()
            .map_err(|error| error.to_string())?;
        if response.status().is_redirection() {
            if hop == MEDIA_MAX_REDIRECTS {
                return Err("too many redirects".into());
            }
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| "redirect is missing Location".to_string())?;
            current = current.join(location).map_err(|error| error.to_string())?;
            continue;
        }
        if !response.status().is_success() {
            return Err(format!("HTTP {}", response.status().as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|length| length > max_bytes as u64)
        {
            return Err("workshop image exceeds configured limit".into());
        }
        allow(&current)?;
        let mut body = Vec::new();
        let mut buffer = [0_u8; READ_CHUNK];
        loop {
            let read = response
                .read(&mut buffer)
                .map_err(|error| error.to_string())?;
            if read == 0 {
                break;
            }
            let next = body
                .len()
                .checked_add(read)
                .ok_or_else(|| "workshop image exceeds configured limit".to_string())?;
            if next > max_bytes {
                return Err("workshop image exceeds configured limit".into());
            }
            body.extend_from_slice(&buffer[..read]);
        }
        return Ok((current, body));
    }
    Err("too many redirects".into())
}

pub struct WorkshopRunner {
    descriptor: RunnerDescriptor,
    transport: Box<dyn WorkshopTransport>,
    resources: Arc<dyn ResourceRegistryGateway>,
    media_provider_id: String,
}
impl WorkshopRunner {
    pub fn new(
        transport: Box<dyn WorkshopTransport>,
        resources: Arc<dyn ResourceRegistryGateway>,
        media_provider_id: impl Into<String>,
    ) -> Self {
        Self {
            descriptor: descriptor(),
            transport,
            resources,
            media_provider_id: media_provider_id.into(),
        }
    }
    fn run_task(&mut self, task: &Task) -> Result<RunnerResult, RuntimeError> {
        let request: WorkshopResolveRequest = serde_json::from_value(task.payload.clone().into())
            .map_err(|error| failure(task, error))?;
        let card = self
            .transport
            .resolve(&request.url)
            .map_err(|error| failure(task, error))?;
        let mut segments = Vec::new();
        if let Some(image_url) = card.image_url {
            let bytes = self
                .transport
                .download(&image_url, MAX_LINK_CARD_MEDIA_BYTES)
                .map_err(|error| failure(task, error))?;
            let resource = self
                .resources
                .create_blob_resource(
                    &self.media_provider_id,
                    "mutsuki.bot.image.original.v1",
                    bytes,
                )
                .map_err(|error| failure(task, error))?;
            segments.push(MessageSegment::Image { resource });
        }
        segments.push(MessageSegment::Text {
            text: format!("{}\n{}\n{}", card.title, card.description, card.url),
        });
        let message = BotMessage {
            message_id: None,
            target: request.target,
            sender: None,
            segments,
            reply_to: None,
            time_ms: None,
            ext: BotExtMap::new(),
        };
        let mut outbound = Task::new(
            format!("{}:notify", task.task_id),
            BOT_MESSAGE_SEND_PROTOCOL_ID,
            serde_json::to_value(message).expect("message serializes"),
        );
        outbound.target_binding_id = Some(request.outbound_binding);
        let mut result = RunnerResult::completed(task.task_id.clone());
        result.tasks.push(outbound);
        Ok(result)
    }
}
impl Runner for WorkshopRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }
    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        map_work_batch_entries(&batch, |task| self.run_task(task))
    }
}

pub fn manifest() -> mutsuki_runtime_contracts::PluginManifest {
    let mut manifest = PluginBuilder::new(PLUGIN_ID)
        .runner(Box::new(ManifestRunner {
            descriptor: descriptor(),
        }))
        .protocol_handler(
            ProtocolDescriptorBuilder::new(LINK_RESOLVE)
                .input_schema(json!({"type":"object"}))
                .output_schema(json!({"type":"object"}))
                .error_schema(json!({"type":"object"}))
                .build(),
            RUNNER_ID,
            "io",
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
        .purity(RunnerPurity::Effectful)
        .execution_class(ExecutionClass::Io)
        .metadata("domain", ScalarValue::String("bilibili_workshop".into()))
        .build()
}
struct ManifestRunner {
    descriptor: RunnerDescriptor,
}
impl Runner for ManifestRunner {
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
            RuntimeError::new("runner.unavailable", PLUGIN_ID, "manifest_only"),
        ))
    }
}
fn allow_workshop_url(url: &Url) -> Result<(), String> {
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
fn failure(task: &Task, detail: impl std::fmt::Display) -> RuntimeError {
    let mut error = RuntimeError::new(
        "bilibili.workshop.resolve_failed",
        PLUGIN_ID,
        format!("workshop.{}", task.task_id),
    );
    error
        .evidence
        .insert("detail".into(), ScalarValue::String(detail.to_string()));
    error
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    use super::*;

    fn loopback(url: &Url) -> Result<(), String> {
        let host = url.host_str().unwrap_or_default();
        if url.scheme() == "http" && host == "127.0.0.1" {
            Ok(())
        } else {
            Err(format!("workshop domain denied: {host}"))
        }
    }

    fn client() -> Client {
        let _ = rustls::crypto::ring::default_provider().install_default();
        Client::builder()
            .timeout(Duration::from_secs(2))
            .redirect(Policy::none())
            .build()
            .unwrap()
    }

    fn serve_once(payload: &[u8]) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let payload = payload.to_vec();
        let join = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0_u8; 4096];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(&payload);
        });
        (format!("http://{address}"), join)
    }

    #[test]
    fn manifest_and_domain_policy() {
        assert_eq!(
            manifest().provides.protocol_classes.get(LINK_RESOLVE),
            Some(&ProtocolClass::Effect)
        );
        assert!(allow_workshop_url(&Url::parse("https://evil.example/item").unwrap()).is_err());
        assert!(
            allow_workshop_url(&Url::parse("https://www.bilibili.com/video/BV1").unwrap()).is_err()
        );
        assert!(allow_workshop_url(&Url::parse("http://mall.bilibili.com/item").unwrap()).is_err());
        assert!(allow_workshop_url(&Url::parse("https://mall.bilibili.com/item").unwrap()).is_ok());
    }

    #[test]
    fn fetch_enforces_redirect_allowlist_stream_limit_and_success() {
        let (base, join) = serve_once(
            b"HTTP/1.1 302 Found\r\nConnection: close\r\nLocation: http://evil.example/x\r\n\r\n",
        );
        let error = fetch_bytes(&client(), &format!("{base}/r"), 1024, loopback).unwrap_err();
        join.join().unwrap();
        assert!(error.contains("domain denied"));

        let (base, join) = serve_once(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nxxxxxxxx");
        let error = fetch_bytes(&client(), &format!("{base}/big"), 4, loopback).unwrap_err();
        join.join().unwrap();
        assert!(error.contains("exceeds configured limit"));

        let (base, join) = serve_once(
            b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 8\r\n\r\nok-bytes",
        );
        let (_, bytes) = fetch_bytes(&client(), &format!("{base}/ok"), 1024, loopback).unwrap();
        join.join().unwrap();
        assert_eq!(bytes, b"ok-bytes");
    }
}
