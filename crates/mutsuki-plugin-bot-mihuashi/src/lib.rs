use std::sync::Arc;

use mutsuki_bot_protocol::{
    BOT_MESSAGE_SEND_PROTOCOL_ID, BotExtMap, BotMessage, BotTarget, MessageSegment,
};
use mutsuki_protocol_browser::{
    BrowserSnapshot, BrowserSnapshotRequest, BrowserWaitMode, SNAPSHOT, SNAPSHOT_SCHEMA,
};
use mutsuki_runtime_contracts::{
    ExecutionClass, ReadPlan, RunnerDescriptor, RunnerPurity, RunnerResult, RuntimeError,
    ScalarValue, Task, TaskOutcome,
};
use mutsuki_runtime_core::{Runner, RuntimeFailure, RuntimeResult};
use mutsuki_runtime_sdk::{
    AsyncRunnerAdapter, PluginBuilder, ProtocolDescriptorBuilder, ResourceRegistryGateway,
    RunnerDescriptorBuilder, RuntimeClientRef,
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
    max_media_bytes: usize,
) -> Box<dyn Runner> {
    let descriptor = descriptor();
    let factory = Box::new(
        move |ctx: mutsuki_runtime_sdk::AsyncRunnerContext, task: Task| {
            let resources = resources.clone();
            let media_provider_id = media_provider_id.clone();
            Box::pin(async move {
                run_task(ctx, task, resources, media_provider_id, max_media_bytes).await
            })
                as std::pin::Pin<
                    Box<dyn std::future::Future<Output = RuntimeResult<RunnerResult>> + Send>,
                >
        },
    );
    Box::new(AsyncRunnerAdapter::new(descriptor, client, factory).with_self_call_policy(false))
}

async fn run_task(
    ctx: mutsuki_runtime_sdk::AsyncRunnerContext,
    task: Task,
    resources: Arc<dyn ResourceRegistryGateway>,
    media_provider_id: String,
    max_media_bytes: usize,
) -> RuntimeResult<RunnerResult> {
    let request: MihuashiResolveRequest =
        serde_json::from_value(task.payload.clone()).map_err(|error| fail(&task, error))?;
    ensure_mihuashi_url(&request.url).map_err(|error| fail(&task, error))?;
    let output = resources.create_cow_state_resource(
        &media_provider_id,
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
                selector: Some(request.selector),
                timeout_ms: request.timeout_ms,
            })
            .map_err(|error| fail(&task, error))?,
        )
        .await?;
    if !matches!(outcome, TaskOutcome::Completed { .. }) {
        return Err(fail(&task, "browser snapshot child task failed"));
    }
    let latest = resources.open_resource_descriptor(&output.ref_id)?;
    let bytes = resources.collect_read_plan(&ReadPlan {
        plan_id: format!("mihuashi.snapshot.read.{}", task.task_id),
        resource: latest,
        operation: "collect".into(),
        args: Value::Null,
    })?;
    let snapshot: BrowserSnapshot =
        serde_json::from_slice(&bytes).map_err(|error| fail(&task, error))?;
    let card =
        parse_profile(&snapshot.html, &snapshot.final_url).map_err(|error| fail(&task, error))?;
    let mut segments = Vec::new();
    if let Some(image_url) = card.2 {
        ensure_mihuashi_url(&image_url).map_err(|error| fail(&task, error))?;
        let bytes = reqwest::get(&image_url)
            .await
            .map_err(|error| fail(&task, error))?
            .bytes()
            .await
            .map_err(|error| fail(&task, error))?;
        if bytes.len() > max_media_bytes {
            return Err(fail(&task, "Mihuashi image exceeds configured limit"));
        }
        let resource = resources.create_blob_resource(
            &media_provider_id,
            "mutsuki.bot.image.original.v1",
            bytes.to_vec(),
        )?;
        segments.push(MessageSegment::Image { resource });
    }
    segments.push(MessageSegment::Text {
        text: format!("{}\n{}\n{}", card.0, card.1, snapshot.final_url),
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
        serde_json::to_value(message).map_err(|error| fail(&task, error))?,
    );
    outbound.target_binding_id = Some(request.outbound_binding);
    let mut result = RunnerResult::completed(task.task_id);
    result.tasks.push(outbound);
    Ok(result)
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

pub fn manifest() -> mutsuki_runtime_contracts::PluginManifest {
    PluginBuilder::new(PLUGIN_ID)
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
        .manifest
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
    use super::*;
    #[test]
    fn parses_server_rendered_fixture() {
        let html = "<html><head><meta property='og:image' content='https://img.mihuashi.com/a.jpg'></head><body><h1>Painter</h1><main>Window</main></body></html>";
        let parsed = parse_profile(html, "https://www.mihuashi.com/profiles/1").unwrap();
        assert_eq!(parsed.0, "Painter");
        assert_eq!(parsed.1, "Window");
    }
}
