use serde::{Deserialize, Serialize};

use crate::ResourceRef;

/// Neutral multimodal part carried beside plain `AgentMessage.content`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentContentPart {
    Text {
        text: String,
    },
    Image {
        resource: ResourceRef,
        #[serde(default)]
        mime_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size_bytes: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provenance: Option<String>,
    },
    Audio {
        resource: ResourceRef,
        #[serde(default)]
        mime_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size_bytes: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provenance: Option<String>,
    },
    Document {
        resource: ResourceRef,
        #[serde(default)]
        mime_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size_bytes: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provenance: Option<String>,
    },
    /// Remote URL input is allowed only when the active permission policy permits it.
    RemoteUrl {
        url: String,
        #[serde(default)]
        mime_type: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration_ms: Option<u64>,
    #[serde(default)]
    pub allowed_mime_types: Vec<String>,
    #[serde(default)]
    pub allow_remote_url: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TranscriptionRequest {
    pub audio: ResourceRef,
    #[serde(default)]
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_hint: Option<String>,
    #[serde(default)]
    pub timestamps: bool,
    #[serde(default)]
    pub diarization: bool,
    #[serde(default)]
    pub stream: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TranscriptionEvent {
    Partial {
        sequence: u64,
        text: String,
    },
    Segment {
        sequence: u64,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        start_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        end_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        speaker: Option<String>,
    },
    Completed {
        sequence: u64,
        result: Box<TranscriptionResult>,
    },
    Failed {
        sequence: u64,
        code: String,
        message: String,
    },
    Cancelled {
        sequence: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TranscriptionResult {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default)]
    pub segments: Vec<TranscriptionSegment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_ref: Option<ResourceRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptionSegment {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpeechSynthesisRequest {
    pub text: String,
    #[serde(default)]
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate_hz: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SpeechSynthesisEvent {
    AudioDelta {
        sequence: u64,
        chunk: ResourceRef,
    },
    Completed {
        sequence: u64,
        result: SpeechSynthesisResult,
    },
    Failed {
        sequence: u64,
        code: String,
        message: String,
    },
    Cancelled {
        sequence: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpeechSynthesisResult {
    pub audio: ResourceRef,
    #[serde(default)]
    pub format: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate_hz: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// Host-provided media transform boundary. AgentKit never embeds codecs.
pub trait MediaService: Send + Sync {
    fn transcode(
        &self,
        source: &ResourceRef,
        target_mime: &str,
    ) -> Result<ResourceRef, crate::AgentError>;

    fn resample(
        &self,
        source: &ResourceRef,
        sample_rate_hz: u32,
    ) -> Result<ResourceRef, crate::AgentError>;
}
