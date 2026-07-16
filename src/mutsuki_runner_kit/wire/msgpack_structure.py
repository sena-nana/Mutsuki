from __future__ import annotations

from mutsuki_runner_kit.wire.protocol import WireProtocolFailure

MAX_MSGPACK_NESTING_DEPTH = 64
MAX_MSGPACK_CONTAINER_ITEMS = 65_536


def validate_messagepack_structure(data: bytes) -> None:
    offset = _parse_value(data, 0, 0)
    if offset != len(data):
        raise WireProtocolFailure(
            "wire.msgpack_trailing", "trailing bytes after MessagePack value"
        )


def _parse_value(data: bytes, offset: int, depth: int) -> int:
    if depth > MAX_MSGPACK_NESTING_DEPTH:
        raise WireProtocolFailure(
            "wire.msgpack_depth", "MessagePack nesting depth exceeded"
        )
    marker, offset = _read_uint(data, offset, 1)
    if marker <= 0x7F or marker >= 0xE0 or marker in (0xC0, 0xC2, 0xC3):
        return offset
    if 0x80 <= marker <= 0x8F:
        return _parse_container(data, offset, marker & 0x0F, depth, is_map=True)
    if 0x90 <= marker <= 0x9F:
        return _parse_container(data, offset, marker & 0x0F, depth, is_map=False)
    if 0xA0 <= marker <= 0xBF:
        return _skip(data, offset, marker & 0x1F)
    if marker == 0xC1:
        raise WireProtocolFailure("wire.msgpack_malformed", "reserved MessagePack marker")
    if marker in (0xC4, 0xD9):
        length, offset = _read_uint(data, offset, 1)
        return _skip(data, offset, length)
    if marker in (0xC5, 0xDA):
        length, offset = _read_uint(data, offset, 2)
        return _skip(data, offset, length)
    if marker in (0xC6, 0xDB):
        length, offset = _read_uint(data, offset, 4)
        return _skip(data, offset, length)
    if marker in (0xC7, 0xC8, 0xC9):
        width = {0xC7: 1, 0xC8: 2, 0xC9: 4}[marker]
        length, offset = _read_uint(data, offset, width)
        return _skip(data, offset, length + 1)
    scalar_widths = {
        0xCA: 4,
        0xCB: 8,
        0xCC: 1,
        0xCD: 2,
        0xCE: 4,
        0xCF: 8,
        0xD0: 1,
        0xD1: 2,
        0xD2: 4,
        0xD3: 8,
        0xD4: 2,
        0xD5: 3,
        0xD6: 5,
        0xD7: 9,
        0xD8: 17,
    }
    if marker in scalar_widths:
        return _skip(data, offset, scalar_widths[marker])
    if marker in (0xDC, 0xDD):
        count, offset = _read_uint(data, offset, 2 if marker == 0xDC else 4)
        return _parse_container(data, offset, count, depth, is_map=False)
    if marker in (0xDE, 0xDF):
        count, offset = _read_uint(data, offset, 2 if marker == 0xDE else 4)
        return _parse_container(data, offset, count, depth, is_map=True)
    raise WireProtocolFailure("wire.msgpack_malformed", f"unknown marker {marker:#04x}")


def _parse_container(
    data: bytes, offset: int, count: int, depth: int, *, is_map: bool
) -> int:
    if count > MAX_MSGPACK_CONTAINER_ITEMS:
        raise WireProtocolFailure(
            "wire.msgpack_container_limit",
            f"MessagePack container {count} > {MAX_MSGPACK_CONTAINER_ITEMS}",
        )
    for _ in range(count * (2 if is_map else 1)):
        offset = _parse_value(data, offset, depth + 1)
    return offset


def _read_uint(data: bytes, offset: int, width: int) -> tuple[int, int]:
    end = offset + width
    if end > len(data):
        raise WireProtocolFailure("wire.msgpack_malformed", "truncated MessagePack value")
    return int.from_bytes(data[offset:end], "big"), end


def _skip(data: bytes, offset: int, length: int) -> int:
    end = offset + length
    if end > len(data):
        raise WireProtocolFailure("wire.msgpack_malformed", "truncated MessagePack value")
    return end
