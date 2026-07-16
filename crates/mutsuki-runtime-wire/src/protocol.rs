use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use mutsuki_runtime_contracts::PluginManifest;

pub const DEBUG_JSONL_CODEC_ID: &str = "mutsuki.codec.typed-jsonl.v1";
pub const BINARY_CODEC_ID: &str = "mutsuki.codec.typed-msgpack.v1";
pub const SCHEMA_REVISION: &str = "mutsuki.runtime.wire/1.2.0";
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_JSONL_LINE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_INLINE_RESOURCE_BYTES: usize = 64 * 1024;
pub const MAX_IN_FLIGHT_REQUESTS: usize = 64;
pub const MANAGEMENT_RESERVED_REQUESTS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

impl WireProtocolVersion {
    pub const CURRENT: Self = Self { major: 1, minor: 2 };

    pub fn ensure_compatible(self) -> Result<(), WireCodecError> {
        if self.major != Self::CURRENT.major {
            return Err(WireCodecError::VersionMismatch {
                expected_major: Self::CURRENT.major,
                actual_major: self.major,
            });
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WireLimits {
    pub max_frame_bytes: usize,
    pub max_payload_bytes: usize,
    pub max_jsonl_line_bytes: usize,
    pub max_inline_resource_bytes: usize,
    pub max_in_flight_requests: usize,
    pub management_reserved_requests: usize,
}

impl WireLimits {
    pub fn validate(self) -> Result<(), WireCodecError> {
        if self.max_frame_bytes == 0
            || self.max_payload_bytes == 0
            || self.max_jsonl_line_bytes == 0
            || self.max_inline_resource_bytes == 0
            || self.max_payload_bytes > self.max_frame_bytes
            || self.max_inline_resource_bytes > self.max_payload_bytes
            || self.max_in_flight_requests < 2
            || self.management_reserved_requests == 0
            || self.management_reserved_requests >= self.max_in_flight_requests
        {
            return Err(WireCodecError::LimitMismatch);
        }
        Ok(())
    }
}

pub const DEFAULT_WIRE_LIMITS: WireLimits = WireLimits {
    max_frame_bytes: MAX_FRAME_BYTES,
    max_payload_bytes: MAX_PAYLOAD_BYTES,
    max_jsonl_line_bytes: MAX_JSONL_LINE_BYTES,
    max_inline_resource_bytes: MAX_INLINE_RESOURCE_BYTES,
    max_in_flight_requests: MAX_IN_FLIGHT_REQUESTS,
    management_reserved_requests: MANAGEMENT_RESERVED_REQUESTS,
};

#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Opcode {
    PluginInitialize = 0x0001,
    RunnerRunBatch = 0x1001,
    RunnerCancel = 0x1002,
    RunnerDispose = 0x1003,
    TaskSubmitBatch = 0x2001,
    TaskCancel = 0x2002,
    TaskOutcome = 0x2003,
    ResourceReadCollect = 0x3001,
    ResourceReadSnapshot = 0x3002,
    ResourceStreamOpen = 0x3003,
    ResourceExport = 0x3004,
    ResourceWriteCommit = 0x3005,
    ResourceCommand = 0x3006,
    ResourceCommandBatch = 0x3007,
    ResourceSaga = 0x3008,
    ResourceCreateBlob = 0x3009,
    ResourceCreateCowState = 0x300a,
    ResourceCreateCapability = 0x300b,
}

impl Opcode {
    pub const ALL: [Self; 18] = [
        Self::PluginInitialize,
        Self::RunnerRunBatch,
        Self::RunnerCancel,
        Self::RunnerDispose,
        Self::TaskSubmitBatch,
        Self::TaskCancel,
        Self::TaskOutcome,
        Self::ResourceReadCollect,
        Self::ResourceReadSnapshot,
        Self::ResourceStreamOpen,
        Self::ResourceExport,
        Self::ResourceWriteCommit,
        Self::ResourceCommand,
        Self::ResourceCommandBatch,
        Self::ResourceSaga,
        Self::ResourceCreateBlob,
        Self::ResourceCreateCowState,
        Self::ResourceCreateCapability,
    ];

    pub const fn method(self) -> &'static str {
        match self {
            Self::PluginInitialize => "plugin.initialize",
            Self::RunnerRunBatch => "runner.run_batch",
            Self::RunnerCancel => "runner.cancel",
            Self::RunnerDispose => "runner.dispose",
            Self::TaskSubmitBatch => "task.submit_batch",
            Self::TaskCancel => "task.cancel",
            Self::TaskOutcome => "task.outcome",
            Self::ResourceReadCollect => "resource.read.collect",
            Self::ResourceReadSnapshot => "resource.read.snapshot",
            Self::ResourceStreamOpen => "resource.stream.open",
            Self::ResourceExport => "resource.export",
            Self::ResourceWriteCommit => "resource.write.commit",
            Self::ResourceCommand => "resource.command",
            Self::ResourceCommandBatch => "resource.command_batch",
            Self::ResourceSaga => "resource.saga",
            Self::ResourceCreateBlob => "resource.create_blob",
            Self::ResourceCreateCowState => "resource.create_cow_state",
            Self::ResourceCreateCapability => "resource.create_capability",
        }
    }

    pub const fn is_management(self) -> bool {
        matches!(
            self,
            Self::PluginInitialize | Self::RunnerCancel | Self::RunnerDispose
        )
    }

    pub fn from_u16(value: u16) -> Result<Self, WireCodecError> {
        Self::ALL
            .into_iter()
            .find(|opcode| *opcode as u16 == value)
            .ok_or(WireCodecError::UnknownOpcode(value))
    }

    pub fn from_method(value: &str) -> Result<Self, WireCodecError> {
        Self::ALL
            .into_iter()
            .find(|opcode| opcode.method() == value)
            .ok_or_else(|| WireCodecError::UnknownMethod(value.into()))
    }
}

pub trait WireRequest: Serialize {
    const OPCODE: Opcode;
    type Response: Serialize + DeserializeOwned;

    fn validate(&self, _limits: WireLimits) -> Result<(), WireCodecError> {
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolHello {
    pub protocol: WireProtocolVersion,
    pub codec_id: String,
    pub schema_revision: String,
    pub max_frame_bytes: u32,
    pub max_payload_bytes: u32,
    pub max_in_flight_requests: u32,
    pub management_reserved_requests: u32,
    pub management_channel: bool,
    pub feature_flags: Vec<String>,
}

impl ProtocolHello {
    pub fn debug_jsonl() -> Self {
        Self::for_codec(DEBUG_JSONL_CODEC_ID, DEFAULT_WIRE_LIMITS)
    }

    pub fn binary() -> Self {
        Self::for_codec(BINARY_CODEC_ID, DEFAULT_WIRE_LIMITS)
    }

    pub fn debug_jsonl_with_limits(limits: WireLimits) -> Result<Self, WireCodecError> {
        limits.validate()?;
        Ok(Self::for_codec(DEBUG_JSONL_CODEC_ID, limits))
    }

    fn for_codec(codec_id: &str, limits: WireLimits) -> Self {
        Self {
            protocol: WireProtocolVersion::CURRENT,
            codec_id: codec_id.into(),
            schema_revision: SCHEMA_REVISION.into(),
            max_frame_bytes: limits.max_frame_bytes.min(u32::MAX as usize) as u32,
            max_payload_bytes: limits.max_payload_bytes.min(u32::MAX as usize) as u32,
            max_in_flight_requests: limits.max_in_flight_requests.min(u32::MAX as usize) as u32,
            management_reserved_requests: limits.management_reserved_requests.min(u32::MAX as usize)
                as u32,
            management_channel: true,
            feature_flags: vec![
                "typed_requests".into(),
                "out_of_order_responses".into(),
                "resource_ref_required_for_large_bytes".into(),
            ],
        }
    }

    pub fn accept(
        &self,
        expected_codec: &str,
        plugin: Option<InitializedPlugin>,
    ) -> Result<ProtocolHelloAck, WireCodecError> {
        self.protocol.ensure_compatible()?;
        if self.codec_id != expected_codec {
            return Err(WireCodecError::CodecMismatch {
                expected: expected_codec.into(),
                actual: self.codec_id.clone(),
            });
        }
        if self.schema_revision != SCHEMA_REVISION {
            return Err(WireCodecError::SchemaMismatch {
                expected: SCHEMA_REVISION.into(),
                actual: self.schema_revision.clone(),
            });
        }
        if self.max_frame_bytes == 0
            || self.max_payload_bytes == 0
            || self.max_in_flight_requests == 0
            || self.management_reserved_requests == 0
            || self.management_reserved_requests >= self.max_in_flight_requests
        {
            return Err(WireCodecError::LimitMismatch);
        }
        let required = ProtocolHello::for_codec(expected_codec, DEFAULT_WIRE_LIMITS);
        if required.management_channel && !self.management_channel {
            return Err(WireCodecError::ManagementChannelRequired);
        }
        if let Some(missing) = required
            .feature_flags
            .iter()
            .find(|required| !self.feature_flags.contains(required))
        {
            return Err(WireCodecError::FeatureMissing(missing.clone()));
        }
        Ok(ProtocolHelloAck::accept(self, plugin))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProtocolHelloAck {
    pub protocol: WireProtocolVersion,
    pub codec_id: String,
    pub schema_revision: String,
    pub max_frame_bytes: u32,
    pub max_payload_bytes: u32,
    pub max_in_flight_requests: u32,
    pub management_reserved_requests: u32,
    pub management_channel: bool,
    pub feature_flags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<InitializedPlugin>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InitializedPlugin {
    pub manifest: PluginManifest,
    pub resource_provider_ids: Vec<String>,
}

impl ProtocolHelloAck {
    pub fn accept(hello: &ProtocolHello, plugin: Option<InitializedPlugin>) -> Self {
        Self {
            protocol: WireProtocolVersion::CURRENT,
            codec_id: hello.codec_id.clone(),
            schema_revision: hello.schema_revision.clone(),
            max_frame_bytes: hello.max_frame_bytes.min(MAX_FRAME_BYTES as u32),
            max_payload_bytes: hello.max_payload_bytes.min(MAX_PAYLOAD_BYTES as u32),
            max_in_flight_requests: hello
                .max_in_flight_requests
                .min(MAX_IN_FLIGHT_REQUESTS as u32),
            management_reserved_requests: hello
                .management_reserved_requests
                .min(MANAGEMENT_RESERVED_REQUESTS as u32)
                .min(hello.max_in_flight_requests.saturating_sub(1)),
            management_channel: hello.management_channel,
            feature_flags: hello.feature_flags.clone(),
            plugin,
        }
    }

    pub fn validate_for(&self, hello: &ProtocolHello) -> Result<(), WireCodecError> {
        self.protocol.ensure_compatible()?;
        if self.codec_id != hello.codec_id {
            return Err(WireCodecError::CodecMismatch {
                expected: hello.codec_id.clone(),
                actual: self.codec_id.clone(),
            });
        }
        if self.schema_revision != hello.schema_revision {
            return Err(WireCodecError::SchemaMismatch {
                expected: hello.schema_revision.clone(),
                actual: self.schema_revision.clone(),
            });
        }
        if self.max_frame_bytes == 0
            || self.max_frame_bytes > hello.max_frame_bytes
            || self.max_payload_bytes == 0
            || self.max_payload_bytes > hello.max_payload_bytes
            || self.max_in_flight_requests == 0
            || self.max_in_flight_requests > hello.max_in_flight_requests
            || self.management_reserved_requests == 0
            || self.management_reserved_requests > hello.management_reserved_requests
            || self.management_reserved_requests >= self.max_in_flight_requests
        {
            return Err(WireCodecError::LimitMismatch);
        }
        if hello.management_channel && !self.management_channel {
            return Err(WireCodecError::ManagementChannelRequired);
        }
        if let Some(missing) = hello
            .feature_flags
            .iter()
            .find(|required| !self.feature_flags.contains(required))
        {
            return Err(WireCodecError::FeatureMissing(missing.clone()));
        }
        Ok(())
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum WireCodecError {
    #[error("wire protocol major mismatch: expected {expected_major}, got {actual_major}")]
    VersionMismatch {
        expected_major: u16,
        actual_major: u16,
    },
    #[error("wire codec mismatch: expected {expected}, got {actual}")]
    CodecMismatch { expected: String, actual: String },
    #[error("wire schema mismatch: expected {expected}, got {actual}")]
    SchemaMismatch { expected: String, actual: String },
    #[error("wire peer returned invalid or expanded negotiated limits")]
    LimitMismatch,
    #[error("wire management channel support is required")]
    ManagementChannelRequired,
    #[error("wire peer is missing required feature {0}")]
    FeatureMissing(String),
    #[error("unknown wire opcode {0:#06x}")]
    UnknownOpcode(u16),
    #[error("unknown wire method {0}")]
    UnknownMethod(String),
    #[error("opcode {opcode:#06x} maps to {expected}, not {actual}")]
    MethodMismatch {
        opcode: u16,
        expected: String,
        actual: String,
    },
    #[error("wire payload is oversized: {actual} > {limit}")]
    PayloadOversized { actual: usize, limit: usize },
    #[error("wire frame is oversized: {actual} > {limit}")]
    FrameOversized { actual: usize, limit: usize },
    #[error("wire frame is truncated: expected {expected}, got {actual}")]
    Truncated { expected: usize, actual: usize },
    #[error("invalid wire magic {0:#010x}")]
    InvalidMagic(u32),
    #[error("invalid wire flags {0:#06x}")]
    InvalidFlags(u16),
    #[error("wire request id must be non-zero")]
    InvalidRequestId,
    #[error("wire payload length mismatch: declared {declared}, actual {actual}")]
    PayloadLengthMismatch { declared: usize, actual: usize },
    #[error("large resource bytes must use ResourceRef: {actual} > {limit}")]
    InlineResourceOversized { actual: usize, limit: usize },
    #[error("wire encode failed: {0}")]
    Encode(String),
    #[error("wire decode failed: {0}")]
    Decode(String),
    #[error("wire I/O failed: {0}")]
    Io(String),
}
