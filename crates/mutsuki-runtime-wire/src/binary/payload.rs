use serde::de::DeserializeOwned;
mod structure;

use serde::Serialize;

use crate::WireCodecError;

use super::BinaryFrame;
use structure::validate_messagepack_structure;

pub const MAX_MSGPACK_NESTING_DEPTH: usize = 64;
pub const MAX_MSGPACK_CONTAINER_ITEMS: usize = 65_536;

pub fn decode_binary_payload<T: DeserializeOwned>(
    frame: &BinaryFrame,
) -> Result<T, WireCodecError> {
    validate_messagepack_structure(frame.payload.as_slice())?;
    let mut deserializer = rmp_serde::Deserializer::new(frame.payload.as_slice());
    deserializer.set_max_depth(MAX_MSGPACK_NESTING_DEPTH);
    serde::Deserialize::deserialize(&mut deserializer)
        .map_err(|error| WireCodecError::Decode(error.to_string()))
}

pub(super) fn encode_messagepack<T: Serialize>(value: &T) -> Result<Vec<u8>, WireCodecError> {
    let mut payload = Vec::new();
    let mut serializer = rmp_serde::Serializer::new(&mut payload).with_struct_map();
    serializer.unstable_set_max_depth(MAX_MSGPACK_NESTING_DEPTH);
    value
        .serialize(&mut serializer)
        .map_err(|error| WireCodecError::Encode(error.to_string()))?;
    validate_messagepack_structure(&payload)
        .map_err(|error| WireCodecError::Encode(error.to_string()))?;
    Ok(payload)
}
