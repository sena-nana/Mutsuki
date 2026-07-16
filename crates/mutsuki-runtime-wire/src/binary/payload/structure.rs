use crate::WireCodecError;

use super::{MAX_MSGPACK_CONTAINER_ITEMS, MAX_MSGPACK_NESTING_DEPTH};

pub(super) fn validate_messagepack_structure(bytes: &[u8]) -> Result<(), WireCodecError> {
    let mut offset = 0;
    parse_value(bytes, &mut offset, 0)?;
    if offset != bytes.len() {
        return Err(decode_error("trailing bytes after MessagePack value"));
    }
    Ok(())
}

fn parse_value(bytes: &[u8], offset: &mut usize, depth: usize) -> Result<(), WireCodecError> {
    if depth > MAX_MSGPACK_NESTING_DEPTH {
        return Err(decode_error("MessagePack nesting depth exceeded"));
    }
    let marker = read_u8(bytes, offset)?;
    match marker {
        0x00..=0x7f | 0xc0 | 0xc2 | 0xc3 | 0xe0..=0xff => Ok(()),
        0x80..=0x8f => parse_container(bytes, offset, (marker & 0x0f) as usize, depth, true),
        0x90..=0x9f => parse_container(bytes, offset, (marker & 0x0f) as usize, depth, false),
        0xa0..=0xbf => skip(bytes, offset, (marker & 0x1f) as usize),
        0xc1 => Err(decode_error("reserved MessagePack marker")),
        0xc4 | 0xd9 => {
            let length = read_u8(bytes, offset)? as usize;
            skip(bytes, offset, length)
        }
        0xc5 | 0xda => {
            let length = read_u16(bytes, offset)? as usize;
            skip(bytes, offset, length)
        }
        0xc6 | 0xdb => {
            let length = read_u32(bytes, offset)? as usize;
            skip(bytes, offset, length)
        }
        0xc7 => {
            let length = read_u8(bytes, offset)? as usize;
            parse_extension(bytes, offset, length)
        }
        0xc8 => {
            let length = read_u16(bytes, offset)? as usize;
            parse_extension(bytes, offset, length)
        }
        0xc9 => {
            let length = read_u32(bytes, offset)? as usize;
            parse_extension(bytes, offset, length)
        }
        0xca => skip(bytes, offset, 4),
        0xcb => skip(bytes, offset, 8),
        0xcc | 0xd0 => skip(bytes, offset, 1),
        0xcd | 0xd1 => skip(bytes, offset, 2),
        0xce | 0xd2 => skip(bytes, offset, 4),
        0xcf | 0xd3 => skip(bytes, offset, 8),
        0xd4 => parse_extension(bytes, offset, 1),
        0xd5 => parse_extension(bytes, offset, 2),
        0xd6 => parse_extension(bytes, offset, 4),
        0xd7 => parse_extension(bytes, offset, 8),
        0xd8 => parse_extension(bytes, offset, 16),
        0xdc => {
            let count = read_u16(bytes, offset)? as usize;
            parse_container(bytes, offset, count, depth, false)
        }
        0xdd => {
            let count = read_u32(bytes, offset)? as usize;
            parse_container(bytes, offset, count, depth, false)
        }
        0xde => {
            let count = read_u16(bytes, offset)? as usize;
            parse_container(bytes, offset, count, depth, true)
        }
        0xdf => {
            let count = read_u32(bytes, offset)? as usize;
            parse_container(bytes, offset, count, depth, true)
        }
    }
}

fn parse_container(
    bytes: &[u8],
    offset: &mut usize,
    count: usize,
    depth: usize,
    map: bool,
) -> Result<(), WireCodecError> {
    if count > MAX_MSGPACK_CONTAINER_ITEMS {
        return Err(decode_error("MessagePack container item limit exceeded"));
    }
    let values = if map {
        count
            .checked_mul(2)
            .ok_or_else(|| decode_error("MessagePack map length overflow"))?
    } else {
        count
    };
    for _ in 0..values {
        parse_value(bytes, offset, depth + 1)?;
    }
    Ok(())
}

fn parse_extension(bytes: &[u8], offset: &mut usize, length: usize) -> Result<(), WireCodecError> {
    skip(bytes, offset, length.saturating_add(1))
}

fn read_u8(bytes: &[u8], offset: &mut usize) -> Result<u8, WireCodecError> {
    let value = *bytes
        .get(*offset)
        .ok_or_else(|| decode_error("truncated MessagePack value"))?;
    *offset += 1;
    Ok(value)
}

fn read_u16(bytes: &[u8], offset: &mut usize) -> Result<u16, WireCodecError> {
    let slice = take(bytes, offset, 2)?;
    Ok(u16::from_be_bytes([slice[0], slice[1]]))
}

fn read_u32(bytes: &[u8], offset: &mut usize) -> Result<u32, WireCodecError> {
    let slice = take(bytes, offset, 4)?;
    Ok(u32::from_be_bytes(
        slice.try_into().expect("four-byte value"),
    ))
}

fn skip(bytes: &[u8], offset: &mut usize, length: usize) -> Result<(), WireCodecError> {
    take(bytes, offset, length).map(|_| ())
}

fn take<'a>(
    bytes: &'a [u8],
    offset: &mut usize,
    length: usize,
) -> Result<&'a [u8], WireCodecError> {
    let end = offset
        .checked_add(length)
        .ok_or_else(|| decode_error("MessagePack length overflow"))?;
    let slice = bytes
        .get(*offset..end)
        .ok_or_else(|| decode_error("truncated MessagePack value"))?;
    *offset = end;
    Ok(slice)
}

fn decode_error(detail: &'static str) -> WireCodecError {
    WireCodecError::Decode(detail.into())
}
