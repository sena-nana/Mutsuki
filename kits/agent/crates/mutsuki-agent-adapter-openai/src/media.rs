use std::time::Duration;

use mutsuki_agent_adapter_api::{
    MediaProtocolAdapter, ModelProtocolAdapter, SpeechSynthesisFuture, TranscriptionFuture,
};
use mutsuki_agent_contracts::{
    ModelCapability, ModelProtocolAdapterDescriptor, ProtocolError, ProtocolErrorClass,
    ProviderInstanceDescriptor, SpeechSynthesisRequest, SpeechSynthesisResult,
    TranscriptionRequest, TranscriptionResult,
};
use mutsuki_agent_sdk::stream_resource_ref;
use reqwest::Client;
use serde_json::{Value, json};

use crate::{OpenAiCompatibleAdapter, error, transport_error};

pub const MEDIA_OWNER: &str = "mutsuki.agent.adapter.openai-compatible.media";

/// Deterministic fake media adapter for conformance and unit tests.
#[derive(Clone)]
pub struct FakeMediaAdapter {
    descriptor: ModelProtocolAdapterDescriptor,
}

impl FakeMediaAdapter {
    pub fn new() -> Self {
        Self {
            descriptor: ModelProtocolAdapterDescriptor {
                adapter_id: "openai-compatible-media-fake".into(),
                protocol: "openai.audio".into(),
                version: "1".into(),
                runner_id: "mutsuki.agent.adapter.openai-compatible.media.fake".into(),
                capability: ModelCapability {
                    context_window: 0,
                    transcription: true,
                    speech_synthesis: true,
                    audio_input: true,
                    streaming: true,
                    ..ModelCapability::default()
                },
            },
        }
    }
}

impl Default for FakeMediaAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl MediaProtocolAdapter for FakeMediaAdapter {
    fn descriptor(&self) -> &ModelProtocolAdapterDescriptor {
        &self.descriptor
    }

    fn transcribe(
        &self,
        _provider: ProviderInstanceDescriptor,
        request: TranscriptionRequest,
    ) -> TranscriptionFuture {
        Box::pin(async move {
            if request.diarization {
                return Err(unsupported("diarization"));
            }
            let text = format!(
                "transcript:{}:{}",
                request.audio.ref_id,
                request.language.clone().unwrap_or_else(|| "auto".into())
            );
            Ok(TranscriptionResult {
                text,
                language: request.language,
                segments: Vec::new(),
                transcript_ref: Some(stream_resource_ref(MEDIA_OWNER, "transcript-fake")),
            })
        })
    }

    fn synthesize(
        &self,
        _provider: ProviderInstanceDescriptor,
        request: SpeechSynthesisRequest,
    ) -> SpeechSynthesisFuture {
        Box::pin(async move {
            if request.text.trim().is_empty() {
                return Err(error(
                    "agent.adapter.invalid_request",
                    ProtocolErrorClass::NonRetryable,
                    "speech text is required",
                ));
            }
            let format = request.format.clone().unwrap_or_else(|| "wav".into());
            Ok(SpeechSynthesisResult {
                audio: stream_resource_ref(
                    MEDIA_OWNER,
                    format!(
                        "speech-fake-{}",
                        request.voice_id.clone().unwrap_or_default()
                    ),
                ),
                format,
                sample_rate_hz: request.sample_rate_hz.or(Some(24_000)),
                duration_ms: Some((request.text.len() as u64).saturating_mul(40)),
            })
        })
    }
}

impl OpenAiCompatibleAdapter {
    pub fn transcribe_media(
        &self,
        provider: ProviderInstanceDescriptor,
        request: TranscriptionRequest,
    ) -> TranscriptionFuture {
        let adapter = self.clone();
        Box::pin(async move { adapter.transcribe_request(provider, request).await })
    }

    pub fn synthesize_media(
        &self,
        provider: ProviderInstanceDescriptor,
        request: SpeechSynthesisRequest,
    ) -> SpeechSynthesisFuture {
        let adapter = self.clone();
        Box::pin(async move { adapter.synthesize_request(provider, request).await })
    }

    async fn transcribe_request(
        &self,
        provider: ProviderInstanceDescriptor,
        request: TranscriptionRequest,
    ) -> Result<TranscriptionResult, ProtocolError> {
        ensure_capability(&provider, |capability| capability.transcription)?;
        if request.diarization {
            return Err(unsupported("diarization"));
        }
        let endpoint = audio_endpoint(&provider, "transcriptions")?;
        let credential = self
            .credentials
            .resolve(provider.credential.clone())
            .await?;
        let timeout_ms = request.timeout_ms.unwrap_or(30_000);
        let payload = json!({
            "model": if request.model.trim().is_empty() { "whisper-1" } else { &request.model },
            "resource_ref": request.audio.ref_id,
            "language": request.language,
            "prompt": request.prompt_hint,
            "timestamp_granularities": if request.timestamps { json!(["segment"]) } else { Value::Null },
        });
        let body = post_json(
            &self.client,
            endpoint,
            credential.expose_secret(),
            timeout_ms,
            payload,
        )
        .await?;
        let text = body
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                error(
                    "agent.adapter.invalid_response",
                    ProtocolErrorClass::Protocol,
                    "transcription response is missing text",
                )
            })?
            .to_owned();
        Ok(TranscriptionResult {
            text,
            language: request.language,
            segments: Vec::new(),
            transcript_ref: Some(stream_resource_ref(MEDIA_OWNER, request.audio.ref_id)),
        })
    }

    async fn synthesize_request(
        &self,
        provider: ProviderInstanceDescriptor,
        request: SpeechSynthesisRequest,
    ) -> Result<SpeechSynthesisResult, ProtocolError> {
        ensure_capability(&provider, |capability| capability.speech_synthesis)?;
        if request.text.trim().is_empty() {
            return Err(error(
                "agent.adapter.invalid_request",
                ProtocolErrorClass::NonRetryable,
                "speech text is required",
            ));
        }
        let endpoint = audio_endpoint(&provider, "speech")?;
        let credential = self
            .credentials
            .resolve(provider.credential.clone())
            .await?;
        let timeout_ms = request.timeout_ms.unwrap_or(30_000);
        let format = request.format.clone().unwrap_or_else(|| "mp3".into());
        let payload = json!({
            "model": if request.model.trim().is_empty() { "tts-1" } else { &request.model },
            "input": request.text,
            "voice": request.voice_id.clone().unwrap_or_else(|| "alloy".into()),
            "response_format": format,
            "speed": request.speed,
        });
        let _body = post_json(
            &self.client,
            endpoint,
            credential.expose_secret(),
            timeout_ms,
            payload,
        )
        .await?;
        Ok(SpeechSynthesisResult {
            audio: stream_resource_ref(MEDIA_OWNER, format!("speech-{}", provider.provider_id)),
            format,
            sample_rate_hz: request.sample_rate_hz,
            duration_ms: None,
        })
    }
}

impl MediaProtocolAdapter for OpenAiCompatibleAdapter {
    fn descriptor(&self) -> &ModelProtocolAdapterDescriptor {
        ModelProtocolAdapter::descriptor(self)
    }

    fn transcribe(
        &self,
        provider: ProviderInstanceDescriptor,
        request: TranscriptionRequest,
    ) -> TranscriptionFuture {
        self.transcribe_media(provider, request)
    }

    fn synthesize(
        &self,
        provider: ProviderInstanceDescriptor,
        request: SpeechSynthesisRequest,
    ) -> SpeechSynthesisFuture {
        self.synthesize_media(provider, request)
    }
}

fn ensure_capability(
    provider: &ProviderInstanceDescriptor,
    predicate: impl Fn(&ModelCapability) -> bool,
) -> Result<(), ProtocolError> {
    let supported = if provider.models.is_empty() {
        provider
            .compatibility
            .get("media_enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    } else {
        provider.models.values().any(predicate)
    };
    if supported {
        Ok(())
    } else {
        Err(unsupported("media capability"))
    }
}

fn audio_endpoint(
    provider: &ProviderInstanceDescriptor,
    leaf: &str,
) -> Result<reqwest::Url, ProtocolError> {
    let mut endpoint = reqwest::Url::parse(&provider.endpoint).map_err(|_| {
        error(
            "agent.adapter.invalid_endpoint",
            ProtocolErrorClass::NonRetryable,
            "provider endpoint is invalid",
        )
    })?;
    let base = endpoint.path().trim_end_matches('/');
    let trimmed = base
        .trim_end_matches("/chat/completions")
        .trim_end_matches("/audio/transcriptions")
        .trim_end_matches("/audio/speech");
    endpoint.set_path(&format!("{trimmed}/audio/{leaf}"));
    Ok(endpoint)
}

async fn post_json(
    client: &Client,
    endpoint: reqwest::Url,
    secret: &str,
    timeout_ms: u64,
    payload: Value,
) -> Result<Value, ProtocolError> {
    let response = client
        .post(endpoint)
        .bearer_auth(secret)
        .timeout(Duration::from_millis(timeout_ms))
        .json(&payload)
        .send()
        .await
        .map_err(|err| transport_error(&err))?;
    if !response.status().is_success() {
        return Err(error(
            "agent.adapter.http_status",
            ProtocolErrorClass::NonRetryable,
            format!(
                "media endpoint returned HTTP {}",
                response.status().as_u16()
            ),
        ));
    }
    response.json().await.map_err(|err| transport_error(&err))
}

fn unsupported(feature: &str) -> ProtocolError {
    error(
        "agent.adapter.unsupported_capability",
        ProtocolErrorClass::NonRetryable,
        format!("{feature} is not supported by this protocol adapter"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_agent_adapter_api::{CredentialBroker, CredentialValue, MediaProtocolAdapter};
    use mutsuki_agent_contracts::{CredentialRef, ResourceRef};
    use std::collections::BTreeMap;

    struct StaticBroker;
    #[allow(dead_code)]
    impl CredentialBroker for StaticBroker {
        fn resolve(
            &self,
            _credential: CredentialRef,
        ) -> mutsuki_agent_adapter_api::CredentialFuture {
            Box::pin(async { CredentialValue::new("secret") })
        }
    }

    fn audio_ref() -> ResourceRef {
        stream_resource_ref(MEDIA_OWNER, "audio-1")
    }

    #[tokio::test]
    async fn fake_stt_and_tts_are_deterministic() {
        let adapter = FakeMediaAdapter::new();
        let provider = ProviderInstanceDescriptor {
            provider_id: "fake".into(),
            adapter_id: adapter.descriptor().adapter_id.clone(),
            endpoint: "https://example.invalid/v1".into(),
            credential: CredentialRef {
                credential_id: "c".into(),
                revision: 1,
            },
            models: BTreeMap::from([(
                "whisper".into(),
                ModelCapability {
                    transcription: true,
                    speech_synthesis: true,
                    ..ModelCapability::default()
                },
            )]),
            headers: BTreeMap::new(),
            compatibility: BTreeMap::new(),
            remote_execution_allowed: false,
        };
        let first = adapter
            .transcribe(
                provider.clone(),
                TranscriptionRequest {
                    audio: audio_ref(),
                    model: "whisper".into(),
                    language: Some("zh".into()),
                    prompt_hint: None,
                    timestamps: false,
                    diarization: false,
                    stream: false,
                    provider_hint: None,
                    timeout_ms: Some(100),
                },
            )
            .await
            .unwrap();
        let second = adapter
            .transcribe(
                provider.clone(),
                TranscriptionRequest {
                    audio: audio_ref(),
                    model: "whisper".into(),
                    language: Some("zh".into()),
                    prompt_hint: None,
                    timestamps: false,
                    diarization: false,
                    stream: false,
                    provider_hint: None,
                    timeout_ms: Some(100),
                },
            )
            .await
            .unwrap();
        assert_eq!(first, second);
        let speech = adapter
            .synthesize(
                provider,
                SpeechSynthesisRequest {
                    text: "hello".into(),
                    model: "tts".into(),
                    voice_id: Some("alloy".into()),
                    format: Some("wav".into()),
                    sample_rate_hz: Some(16_000),
                    speed: Some(1.0),
                    stream: false,
                    provider_hint: None,
                    timeout_ms: Some(100),
                },
            )
            .await
            .unwrap();
        assert_eq!(speech.format, "wav");
        assert!(!speech.audio.ref_id.is_empty());
    }

    #[tokio::test]
    async fn unsupported_diarization_fails_loud() {
        let adapter = FakeMediaAdapter::new();
        let provider = ProviderInstanceDescriptor {
            provider_id: "fake".into(),
            adapter_id: adapter.descriptor().adapter_id.clone(),
            endpoint: "https://example.invalid/v1".into(),
            credential: CredentialRef {
                credential_id: "c".into(),
                revision: 1,
            },
            models: BTreeMap::new(),
            headers: BTreeMap::new(),
            compatibility: BTreeMap::from([("media_enabled".into(), json!(true))]),
            remote_execution_allowed: false,
        };
        let err = adapter
            .transcribe(
                provider,
                TranscriptionRequest {
                    audio: audio_ref(),
                    model: String::new(),
                    language: None,
                    prompt_hint: None,
                    timestamps: false,
                    diarization: true,
                    stream: false,
                    provider_hint: None,
                    timeout_ms: None,
                },
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, "agent.adapter.unsupported_capability");
    }
}
