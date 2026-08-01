use std::sync::Arc;

use mutsuki_agent_contracts::{
    AGENT_SPEECH_SYNTHESIZE_PROTOCOL, AGENT_TRANSCRIBE_PROTOCOL, MediaService,
    SpeechSynthesisRequest, SpeechSynthesisResult, TranscriptionRequest, TranscriptionResult,
};
use mutsuki_bot_protocol::{
    BOT_MEDIA_SYNTHESIZE_PROTOCOL_ID, BOT_MEDIA_TRANSCRIBE_PROTOCOL_ID, BotMediaKind,
    BotMediaSynthesizeRequest, BotMediaSynthesizeResult, BotMediaTranscribeRequest,
    BotMediaTranscribeResult, BotMediaUploadRequest, BotSpeechReplyPolicy,
};
use mutsuki_runtime_contracts::{ExecutionClass, PluginManifest, RunnerResult, Task, TaskOutcome};
use mutsuki_runtime_core::Runner;
use mutsuki_runtime_sdk::{
    AsyncRunnerContext, BoxedTaskAwaitRunner, PluginBuilder, ProtocolDescriptorBuilder,
    RunnerDescriptorBuilder, RuntimeClientRef, RuntimeFailure, RuntimeResult,
    TaskAwaitRunnerAdapter,
};

pub const BOT_MEDIA_BRIDGE_PLUGIN_ID: &str = "mutsuki.plugin.bot.media";
pub const BOT_MEDIA_BRIDGE_RUNNER_ID: &str = "mutsuki.bot.media.bridge";

#[must_use]
pub fn bot_media_bridge_manifest() -> PluginManifest {
    PluginBuilder::new(BOT_MEDIA_BRIDGE_PLUGIN_ID)
        .runner_descriptor(media_bridge_descriptor())
        .protocol_handler(
            ProtocolDescriptorBuilder::new(BOT_MEDIA_TRANSCRIBE_PROTOCOL_ID).build(),
            BOT_MEDIA_BRIDGE_RUNNER_ID,
            "bot-media-transcribe",
        )
        .protocol_handler(
            ProtocolDescriptorBuilder::new(BOT_MEDIA_SYNTHESIZE_PROTOCOL_ID).build(),
            BOT_MEDIA_BRIDGE_RUNNER_ID,
            "bot-media-synthesize",
        )
        .build()
        .manifest
}

pub fn media_bridge_runner(
    client: RuntimeClientRef,
    media: Arc<dyn MediaService>,
) -> Box<dyn Runner> {
    let factory: BoxedTaskAwaitRunner = Box::new(move |ctx, task| {
        let media = media.clone();
        Box::pin(run_task(ctx, task, media))
    });
    Box::new(
        TaskAwaitRunnerAdapter::new(media_bridge_descriptor(), client, factory)
            .with_self_call_policy(false),
    )
}

fn media_bridge_descriptor() -> mutsuki_runtime_contracts::RunnerDescriptor {
    RunnerDescriptorBuilder::new(BOT_MEDIA_BRIDGE_RUNNER_ID, BOT_MEDIA_BRIDGE_PLUGIN_ID)
        .accepted_protocol(BOT_MEDIA_TRANSCRIBE_PROTOCOL_ID)
        .accepted_protocol(BOT_MEDIA_SYNTHESIZE_PROTOCOL_ID)
        .execution_class(ExecutionClass::Orchestration)
        .build()
}

async fn run_task(
    ctx: AsyncRunnerContext,
    task: Task,
    media: Arc<dyn MediaService>,
) -> RuntimeResult<RunnerResult> {
    match task.protocol_id.as_str() {
        BOT_MEDIA_TRANSCRIBE_PROTOCOL_ID => transcribe(ctx, task).await,
        BOT_MEDIA_SYNTHESIZE_PROTOCOL_ID => synthesize(ctx, task, media).await,
        _ => Err(failure(&task, "protocol.unsupported")),
    }
}

async fn transcribe(ctx: AsyncRunnerContext, task: Task) -> RuntimeResult<RunnerResult> {
    let request: BotMediaTranscribeRequest = decode(&task)?;
    validate_resource(&request.audio, &task)?;
    let outcome = ctx
        .call_raw(
            AGENT_TRANSCRIBE_PROTOCOL,
            serde_json::to_value(TranscriptionRequest {
                audio: request.audio,
                model: String::new(),
                language: request.language,
                prompt_hint: request.prompt_hint,
                timestamps: false,
                diarization: false,
                stream: false,
                provider_hint: request.selector_id,
                timeout_ms: request.timeout_ms,
            })
            .map_err(|_| failure(&task, "transcribe.encode"))?,
        )
        .await?;
    let result: TranscriptionResult = outcome_value(outcome, &task)?;
    let mut completed = RunnerResult::completed(task.task_id);
    completed.output = Some(
        serde_json::to_value(BotMediaTranscribeResult {
            text: result.text,
            language: result.language,
        })
        .map_err(|_| failure_raw("transcribe.output"))?,
    );
    Ok(completed)
}

async fn synthesize(
    ctx: AsyncRunnerContext,
    task: Task,
    media: Arc<dyn MediaService>,
) -> RuntimeResult<RunnerResult> {
    let request: BotMediaSynthesizeRequest = decode(&task)?;
    if request.text.trim().is_empty() {
        return Err(failure(&task, "synthesize.invalid"));
    }
    if request.policy == BotSpeechReplyPolicy::TextOnly {
        let mut completed = RunnerResult::completed(task.task_id);
        completed.output = Some(
            serde_json::to_value(BotMediaSynthesizeResult {
                text: Some(request.text),
                upload: None,
            })
            .map_err(|_| failure_raw("synthesize.output"))?,
        );
        return Ok(completed);
    }
    if request.qq_mime_type.trim().is_empty() {
        return Err(failure(&task, "synthesize.invalid"));
    }
    let outcome = ctx
        .call_raw(
            AGENT_SPEECH_SYNTHESIZE_PROTOCOL,
            serde_json::to_value(SpeechSynthesisRequest {
                text: request.text.clone(),
                model: String::new(),
                voice_id: request.voice_selector_id,
                format: None,
                sample_rate_hz: None,
                speed: None,
                stream: false,
                provider_hint: None,
                timeout_ms: request.timeout_ms,
            })
            .map_err(|_| failure(&task, "synthesize.encode"))?,
        )
        .await?;
    let speech: SpeechSynthesisResult = outcome_value(outcome, &task)?;
    validate_resource(&speech.audio, &task)?;
    let audio = media
        .transcode(&speech.audio, &request.qq_mime_type)
        .map_err(|_| failure(&task, "transcode.failed"))?;
    validate_resource(&audio, &task)?;
    let result = BotMediaSynthesizeResult {
        text: matches!(
            request.policy,
            BotSpeechReplyPolicy::TextOnly | BotSpeechReplyPolicy::TextAndVoice
        )
        .then_some(request.text),
        upload: matches!(
            request.policy,
            BotSpeechReplyPolicy::VoiceOnly | BotSpeechReplyPolicy::TextAndVoice
        )
        .then_some(BotMediaUploadRequest {
            target: request.target,
            kind: BotMediaKind::Audio,
            resource: audio,
            file_name: None,
        }),
    };
    let mut completed = RunnerResult::completed(task.task_id);
    completed.output =
        Some(serde_json::to_value(result).map_err(|_| failure_raw("synthesize.output"))?);
    Ok(completed)
}

fn validate_resource(
    resource: &mutsuki_runtime_contracts::ResourceRef,
    task: &Task,
) -> RuntimeResult<()> {
    if resource.provider_id.trim().is_empty()
        || resource.schema.trim().is_empty()
        || resource.size_hint.is_none()
        || resource.content_hash.as_deref().is_none_or(str::is_empty)
    {
        return Err(failure(task, "resource.unvalidated"));
    }
    Ok(())
}

fn decode<T: serde::de::DeserializeOwned>(task: &Task) -> RuntimeResult<T> {
    serde_json::from_value(task.payload.to_value()).map_err(|_| failure(task, "request.invalid"))
}

fn outcome_value<T: serde::de::DeserializeOwned>(
    outcome: TaskOutcome,
    task: &Task,
) -> RuntimeResult<T> {
    match outcome {
        TaskOutcome::Completed {
            output: Some(output),
            ..
        } => serde_json::from_value(output).map_err(|_| failure(task, "child.output.invalid")),
        TaskOutcome::Failed { error, .. } => Err(RuntimeFailure::new(error)),
        TaskOutcome::Completed { output: None, .. }
        | TaskOutcome::Cancelled { .. }
        | TaskOutcome::Expired { .. }
        | TaskOutcome::DeadLetter { .. } => Err(failure(task, "child.not_completed")),
    }
}

fn failure(task: &Task, route: &str) -> RuntimeFailure {
    failure_raw(&format!("{}.{}", task.task_id, route))
}

fn failure_raw(route: &str) -> RuntimeFailure {
    RuntimeFailure::new(mutsuki_runtime_contracts::RuntimeError::new(
        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
        BOT_MEDIA_BRIDGE_PLUGIN_ID,
        route,
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use mutsuki_agent_contracts::{AgentError, TranscriptionResult};
    use mutsuki_runtime_contracts::{
        BatchEntry, BatchPayload, DispatchLane, OrderingRequirement, ResourceAccess, ResourceId,
        ResourceLifetime, ResourceRef, ResourceSealState, ResourceSemantic, TaskBatch, TaskHandle,
        WorkBatch, WorkResourcePlan,
    };
    use mutsuki_runtime_core::RunnerContext;
    use mutsuki_runtime_sdk::RuntimeClient;

    use super::*;

    #[derive(Default)]
    struct OutcomeClient {
        outcomes: Mutex<BTreeMap<String, TaskOutcome>>,
    }

    impl RuntimeClient for OutcomeClient {
        fn submit_batch(&self, _batch: TaskBatch) -> RuntimeResult<Vec<TaskHandle>> {
            Ok(Vec::new())
        }

        fn task_outcome(&self, handle: &TaskHandle) -> RuntimeResult<Option<TaskOutcome>> {
            Ok(self.outcomes.lock().unwrap().get(&handle.task_id).cloned())
        }
    }

    #[derive(Default)]
    struct FakeMedia {
        calls: Mutex<Vec<(String, String)>>,
    }

    impl MediaService for FakeMedia {
        fn transcode(
            &self,
            source: &ResourceRef,
            target_mime: &str,
        ) -> Result<ResourceRef, AgentError> {
            self.calls
                .lock()
                .unwrap()
                .push((source.ref_id.clone(), target_mime.into()));
            Ok(resource("transcoded", target_mime))
        }

        fn resample(
            &self,
            source: &ResourceRef,
            _sample_rate_hz: u32,
        ) -> Result<ResourceRef, AgentError> {
            Ok(source.clone())
        }
    }

    #[test]
    fn text_only_does_not_call_speech_or_codec_services() {
        let client = Arc::new(OutcomeClient::default());
        let media = Arc::new(FakeMedia::default());
        let mut runner = media_bridge_runner(client, media.clone());
        let task = Task::new(
            "text-only",
            BOT_MEDIA_SYNTHESIZE_PROTOCOL_ID,
            serde_json::to_value(BotMediaSynthesizeRequest {
                target: mutsuki_bot_protocol::BotTarget::User {
                    user_id: "user".into(),
                },
                text: "reply".into(),
                policy: BotSpeechReplyPolicy::TextOnly,
                voice_selector_id: None,
                timeout_ms: None,
                qq_mime_type: String::new(),
            })
            .unwrap(),
        );
        let completed = single_result(runner.run_batch(context(), batch(&task)).unwrap());
        assert!(completed.tasks.is_empty());
        assert_eq!(
            serde_json::from_value::<BotMediaSynthesizeResult>(completed.output.unwrap()).unwrap(),
            BotMediaSynthesizeResult {
                text: Some("reply".into()),
                upload: None,
            }
        );
        assert!(media.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn transcription_and_voice_synthesis_use_public_agent_and_media_contracts() {
        let client = Arc::new(OutcomeClient::default());
        let media = Arc::new(FakeMedia::default());
        let mut runner = media_bridge_runner(client.clone(), media.clone());
        let transcribe = Task::new(
            "transcribe",
            BOT_MEDIA_TRANSCRIBE_PROTOCOL_ID,
            serde_json::to_value(BotMediaTranscribeRequest {
                audio: resource("voice", "audio/silk"),
                language: Some("zh".into()),
                prompt_hint: Some("Mutsuki".into()),
                selector_id: Some("stt-default".into()),
                timeout_ms: Some(5_000),
            })
            .unwrap(),
        );
        let waiting = single_result(runner.run_batch(context(), batch(&transcribe)).unwrap());
        assert_eq!(waiting.tasks[0].protocol_id, AGENT_TRANSCRIBE_PROTOCOL);
        let forwarded: TranscriptionRequest = waiting.tasks[0].payload.decode().unwrap();
        assert_eq!(forwarded.audio.ref_id, "ref-voice");
        assert_eq!(forwarded.provider_hint.as_deref(), Some("stt-default"));
        client.outcomes.lock().unwrap().insert(
            waiting.tasks[0].task_id.clone(),
            TaskOutcome::Completed {
                task_id: waiting.tasks[0].task_id.clone(),
                output: Some(
                    serde_json::to_value(TranscriptionResult {
                        text: "你好".into(),
                        language: Some("zh".into()),
                        segments: Vec::new(),
                        transcript_ref: None,
                    })
                    .unwrap(),
                ),
                output_ref: None,
            },
        );
        let completed = single_result(runner.run_batch(context(), batch(&transcribe)).unwrap());
        let transcript: BotMediaTranscribeResult =
            serde_json::from_value(completed.output.unwrap()).unwrap();
        assert_eq!(transcript.text, "你好");

        let synthesize = Task::new(
            "synthesize",
            BOT_MEDIA_SYNTHESIZE_PROTOCOL_ID,
            serde_json::to_value(BotMediaSynthesizeRequest {
                target: mutsuki_bot_protocol::BotTarget::Group {
                    group_id: "group".into(),
                },
                text: "reply".into(),
                policy: BotSpeechReplyPolicy::TextAndVoice,
                voice_selector_id: Some("voice".into()),
                timeout_ms: Some(5_000),
                qq_mime_type: "audio/silk".into(),
            })
            .unwrap(),
        );
        let waiting = single_result(runner.run_batch(context(), batch(&synthesize)).unwrap());
        assert_eq!(
            waiting.tasks[0].protocol_id,
            AGENT_SPEECH_SYNTHESIZE_PROTOCOL
        );
        client.outcomes.lock().unwrap().insert(
            waiting.tasks[0].task_id.clone(),
            TaskOutcome::Completed {
                task_id: waiting.tasks[0].task_id.clone(),
                output: Some(
                    serde_json::to_value(SpeechSynthesisResult {
                        audio: resource("speech", "audio/wav"),
                        format: "wav".into(),
                        sample_rate_hz: Some(24_000),
                        duration_ms: Some(100),
                    })
                    .unwrap(),
                ),
                output_ref: None,
            },
        );
        let completed = single_result(runner.run_batch(context(), batch(&synthesize)).unwrap());
        let result: BotMediaSynthesizeResult =
            serde_json::from_value(completed.output.unwrap()).unwrap();
        assert_eq!(result.text.as_deref(), Some("reply"));
        assert_eq!(result.upload.unwrap().resource.ref_id, "ref-transcoded");
        assert_eq!(
            media.calls.lock().unwrap().as_slice(),
            &[("ref-speech".into(), "audio/silk".into())]
        );
    }

    fn resource(id: &str, schema: &str) -> ResourceRef {
        ResourceRef {
            ref_id: format!("ref-{id}"),
            resource_id: ResourceId {
                kind_id: "blob".into(),
                slot_id: id.into(),
                generation: 1,
                version: 1,
            },
            semantic: ResourceSemantic::VersionedSnapshot,
            provider_id: "test".into(),
            resource_kind: "blob".into(),
            schema: schema.into(),
            version: 1,
            generation: 1,
            access: ResourceAccess::Inline,
            size_hint: Some(4),
            content_hash: Some(format!("sha256:{id}")),
            lifetime: ResourceLifetime::BorrowedUntilTaskEnd,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        }
    }

    fn context() -> RunnerContext {
        RunnerContext::new(1, 1, "media-test", Vec::<String>::new(), "media-invocation")
            .with_batch("media-batch", 1)
    }

    fn batch(task: &Task) -> WorkBatch {
        WorkBatch {
            batch_id: "media-batch".into(),
            tick_id: "media-tick".into(),
            batch_key: BOT_MEDIA_BRIDGE_RUNNER_ID.into(),
            entries: vec![BatchEntry {
                entry_id: "entry".into(),
                task_id: task.task_id.clone(),
                trace_id: None,
                parent_id: None,
                payload_index: 0,
                resource_requirement_indices: Vec::new(),
                cancel_index: None,
                deadline_tick: None,
                priority: 0,
                lane: DispatchLane::Normal,
                ordering: OrderingRequirement::None,
            }],
            payload: BatchPayload::from_tasks(std::slice::from_ref(task)),
            resource_plan: WorkResourcePlan::empty(),
            task_leases: Vec::new(),
        }
    }

    fn single_result(completion: mutsuki_runtime_contracts::CompletionBatch) -> RunnerResult {
        completion
            .results
            .into_iter()
            .next()
            .unwrap()
            .result
            .unwrap()
    }
}
