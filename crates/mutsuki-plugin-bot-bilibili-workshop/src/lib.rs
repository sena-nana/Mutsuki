use std::sync::Arc;

use mutsuki_bot_link_parser::{MAX_LINK_CARD_MEDIA_BYTES, ResolvedLinkCard};
use mutsuki_bot_protocol::{
    BOT_MESSAGE_SEND_PROTOCOL_ID, BotExtMap, BotMessage, BotTarget, MessageSegment,
};
use mutsuki_runtime_contracts::{
    CompletionBatch, ExecutionClass, RunnerContext, RunnerDescriptor, RunnerPurity, RunnerResult,
    RuntimeError, ScalarValue, Task, WorkBatch,
};
use mutsuki_runtime_core::{Runner, RuntimeResult};
use mutsuki_runtime_sdk::{
    PluginBuilder, ProtocolDescriptorBuilder, ResourceRegistryGateway, RunnerDescriptorBuilder,
    map_work_batch_entries,
};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::json;
use url::Url;

pub const PLUGIN_ID: &str = "mutsuki.bot.bilibili.workshop";
pub const RUNNER_ID: &str = "mutsuki.bot.bilibili.workshop.runner";
pub const LINK_RESOLVE: &str = "mutsuki.bot.bilibili.workshop.link/resolve@1";

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
    client: Option<reqwest::blocking::Client>,
}
impl ReqwestWorkshopTransport {
    pub fn new() -> Self {
        Self::default()
    }
    fn client(&mut self) -> Result<&reqwest::blocking::Client, String> {
        if self.client.is_none() {
            self.client = Some(
                reqwest::blocking::Client::builder()
                    .build()
                    .map_err(|error| error.to_string())?,
            );
        }
        Ok(self.client.as_ref().expect("client initialized"))
    }
}
impl WorkshopTransport for ReqwestWorkshopTransport {
    fn resolve(&mut self, url: &str) -> Result<ResolvedLinkCard, String> {
        ensure_domain(url)?;
        let html = self
            .client()?
            .get(url)
            .send()
            .and_then(|response| response.error_for_status())
            .and_then(|response| response.text())
            .map_err(|error| error.to_string())?;
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
            url: url.into(),
            title: meta("og:title").ok_or("workshop title is missing")?,
            description: meta("og:description").unwrap_or_default(),
            image_url: meta("og:image"),
        })
    }
    fn download(&mut self, url: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
        ensure_domain(url)?;
        let bytes = self
            .client()?
            .get(url)
            .send()
            .and_then(|response| response.error_for_status())
            .and_then(|response| response.bytes())
            .map_err(|error| error.to_string())?;
        if bytes.len() > max_bytes {
            return Err("workshop image exceeds configured limit".into());
        }
        Ok(bytes.to_vec())
    }
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
        let request: WorkshopResolveRequest =
            serde_json::from_value(task.payload.clone()).map_err(|error| failure(task, error))?;
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
    PluginBuilder::new(PLUGIN_ID)
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
        .manifest
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
fn ensure_domain(value: &str) -> Result<(), String> {
    let url = Url::parse(value).map_err(|error| error.to_string())?;
    let host = url.host_str().unwrap_or_default();
    if url.scheme() == "https"
        && (host == "mall.bilibili.com"
            || host.ends_with(".mall.bilibili.com")
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
    use super::*;
    #[test]
    fn rejects_non_workshop_domain() {
        assert!(ensure_domain("https://evil.example/item").is_err());
    }
}
