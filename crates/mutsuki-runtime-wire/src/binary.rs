use std::io::Read;

use mutsuki_runtime_contracts::RuntimeError;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{Opcode, WireCodecError, WireLimits, WireProtocolVersion, WireRequest};

pub const BINARY_LENGTH_PREFIX_LEN: usize = 4;
pub const BINARY_HEADER_LEN: usize = 24;
const WIRE_MAGIC: u32 = 0x4d_55_54_53;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WireFlags(u16);

impl WireFlags {
    pub const REQUEST: Self = Self(0x0001);
    pub const RESPONSE: Self = Self(0x0002);
    pub const ERROR: Self = Self(0x0004);
    pub const MANAGEMENT: Self = Self(0x0008);
    const KNOWN: u16 = Self::REQUEST.0 | Self::RESPONSE.0 | Self::ERROR.0 | Self::MANAGEMENT.0;

    pub const fn bits(self) -> u16 {
        self.0
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    fn from_bits(bits: u16) -> Result<Self, WireCodecError> {
        if bits & !Self::KNOWN != 0 {
            return Err(WireCodecError::InvalidFlags(bits));
        }
        let flags = Self(bits);
        if flags.contains(Self::REQUEST) == flags.contains(Self::RESPONSE) {
            return Err(WireCodecError::InvalidFlags(bits));
        }
        if flags.contains(Self::ERROR) && !flags.contains(Self::RESPONSE) {
            return Err(WireCodecError::InvalidFlags(bits));
        }
        Ok(flags)
    }
}

impl std::ops::BitOr for WireFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WireHeader {
    pub protocol: WireProtocolVersion,
    pub opcode: Opcode,
    pub flags: WireFlags,
    pub request_id: u64,
    pub payload_len: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BinaryFrame {
    pub header: WireHeader,
    pub payload: Vec<u8>,
}

pub fn encode_binary_request<R: WireRequest>(
    request_id: u64,
    request: &R,
    limits: WireLimits,
) -> Result<Vec<u8>, WireCodecError> {
    if request_id == 0 {
        return Err(WireCodecError::InvalidRequestId);
    }
    request.validate(limits)?;
    let payload = rmp_serde::to_vec_named(request)
        .map_err(|error| WireCodecError::Encode(error.to_string()))?;
    let flags = if R::OPCODE.is_management() {
        WireFlags::REQUEST | WireFlags::MANAGEMENT
    } else {
        WireFlags::REQUEST
    };
    encode_frame(R::OPCODE, flags, request_id, payload, limits)
}

pub fn encode_binary_response<T: Serialize>(
    request_id: u64,
    opcode: Opcode,
    response: Result<&T, &RuntimeError>,
    limits: WireLimits,
) -> Result<Vec<u8>, WireCodecError> {
    if request_id == 0 {
        return Err(WireCodecError::InvalidRequestId);
    }
    let (flags, payload) = match response {
        Ok(value) => (
            WireFlags::RESPONSE,
            rmp_serde::to_vec_named(value)
                .map_err(|error| WireCodecError::Encode(error.to_string()))?,
        ),
        Err(error) => (
            WireFlags::RESPONSE | WireFlags::ERROR,
            rmp_serde::to_vec_named(error)
                .map_err(|encode| WireCodecError::Encode(encode.to_string()))?,
        ),
    };
    encode_frame(opcode, flags, request_id, payload, limits)
}

pub fn decode_binary_frame(
    bytes: &[u8],
    limits: WireLimits,
) -> Result<BinaryFrame, WireCodecError> {
    if bytes.len() < BINARY_LENGTH_PREFIX_LEN {
        return Err(WireCodecError::Truncated {
            expected: BINARY_LENGTH_PREFIX_LEN,
            actual: bytes.len(),
        });
    }
    let declared = u32::from_be_bytes(bytes[..4].try_into().expect("four-byte prefix")) as usize;
    validate_frame_length(declared, limits)?;
    let actual = bytes.len() - BINARY_LENGTH_PREFIX_LEN;
    if declared != actual {
        return Err(WireCodecError::Truncated {
            expected: declared,
            actual,
        });
    }
    decode_frame_body(&bytes[BINARY_LENGTH_PREFIX_LEN..], limits)
}

pub fn read_binary_frame<R: Read>(
    reader: &mut R,
    limits: WireLimits,
) -> Result<BinaryFrame, WireCodecError> {
    let mut prefix = [0_u8; BINARY_LENGTH_PREFIX_LEN];
    reader
        .read_exact(&mut prefix)
        .map_err(|error| WireCodecError::Io(error.to_string()))?;
    let declared = u32::from_be_bytes(prefix) as usize;
    validate_frame_length(declared, limits)?;
    let mut body = vec![0_u8; declared];
    reader
        .read_exact(&mut body)
        .map_err(|error| WireCodecError::Io(error.to_string()))?;
    decode_frame_body(&body, limits)
}

pub fn decode_binary_payload<T: DeserializeOwned>(
    frame: &BinaryFrame,
) -> Result<T, WireCodecError> {
    rmp_serde::from_slice(&frame.payload).map_err(|error| WireCodecError::Decode(error.to_string()))
}

fn encode_frame(
    opcode: Opcode,
    flags: WireFlags,
    request_id: u64,
    payload: Vec<u8>,
    limits: WireLimits,
) -> Result<Vec<u8>, WireCodecError> {
    if payload.len() > limits.max_payload_bytes {
        return Err(WireCodecError::PayloadOversized {
            actual: payload.len(),
            limit: limits.max_payload_bytes,
        });
    }
    let body_len = BINARY_HEADER_LEN + payload.len();
    validate_frame_length(body_len, limits)?;
    let mut encoded = Vec::with_capacity(BINARY_LENGTH_PREFIX_LEN + body_len);
    encoded.extend_from_slice(&(body_len as u32).to_be_bytes());
    encoded.extend_from_slice(&WIRE_MAGIC.to_be_bytes());
    encoded.extend_from_slice(&WireProtocolVersion::CURRENT.major.to_be_bytes());
    encoded.extend_from_slice(&WireProtocolVersion::CURRENT.minor.to_be_bytes());
    encoded.extend_from_slice(&(opcode as u16).to_be_bytes());
    encoded.extend_from_slice(&flags.bits().to_be_bytes());
    encoded.extend_from_slice(&request_id.to_be_bytes());
    encoded.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    encoded.extend_from_slice(&payload);
    Ok(encoded)
}

fn decode_frame_body(body: &[u8], limits: WireLimits) -> Result<BinaryFrame, WireCodecError> {
    if body.len() < BINARY_HEADER_LEN {
        return Err(WireCodecError::Truncated {
            expected: BINARY_HEADER_LEN,
            actual: body.len(),
        });
    }
    let magic = u32::from_be_bytes(body[0..4].try_into().expect("magic width"));
    if magic != WIRE_MAGIC {
        return Err(WireCodecError::InvalidMagic(magic));
    }
    let protocol = WireProtocolVersion {
        major: u16::from_be_bytes(body[4..6].try_into().expect("major width")),
        minor: u16::from_be_bytes(body[6..8].try_into().expect("minor width")),
    };
    protocol.ensure_compatible()?;
    let opcode = Opcode::from_u16(u16::from_be_bytes(
        body[8..10].try_into().expect("opcode width"),
    ))?;
    let flags = WireFlags::from_bits(u16::from_be_bytes(
        body[10..12].try_into().expect("flags width"),
    ))?;
    let request_id = u64::from_be_bytes(body[12..20].try_into().expect("request id width"));
    if request_id == 0 {
        return Err(WireCodecError::InvalidRequestId);
    }
    let payload_len = u32::from_be_bytes(body[20..24].try_into().expect("payload length width"));
    if payload_len as usize > limits.max_payload_bytes {
        return Err(WireCodecError::PayloadOversized {
            actual: payload_len as usize,
            limit: limits.max_payload_bytes,
        });
    }
    let payload = &body[BINARY_HEADER_LEN..];
    if payload_len as usize != payload.len() {
        return Err(WireCodecError::PayloadLengthMismatch {
            declared: payload_len as usize,
            actual: payload.len(),
        });
    }
    Ok(BinaryFrame {
        header: WireHeader {
            protocol,
            opcode,
            flags,
            request_id,
            payload_len,
        },
        payload: payload.to_vec(),
    })
}

fn validate_frame_length(length: usize, limits: WireLimits) -> Result<(), WireCodecError> {
    if length > limits.max_frame_bytes {
        return Err(WireCodecError::FrameOversized {
            actual: length,
            limit: limits.max_frame_bytes,
        });
    }
    if length < BINARY_HEADER_LEN {
        return Err(WireCodecError::Truncated {
            expected: BINARY_HEADER_LEN,
            actual: length,
        });
    }
    Ok(())
}
