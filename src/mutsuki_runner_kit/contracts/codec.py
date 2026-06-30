from __future__ import annotations

import json
from collections.abc import Mapping, Sequence
from dataclasses import fields, is_dataclass
from enum import StrEnum
from typing import Any, cast

ScalarValue = str | int | float | bool
JsonValue = None | bool | int | float | str | list["JsonValue"] | dict[str, "JsonValue"]
JsonDict = dict[str, JsonValue]


def as_mapping(data: object, contract: str) -> Mapping[str, object]:
    if not isinstance(data, Mapping):
        raise TypeError(f"{contract} expects a mapping")
    return data


def field_value(data: Mapping[str, object], field_name: str) -> object:
    if field_name not in data:
        raise TypeError(f"{field_name} is required")
    return data[field_name]


def as_str(value: object, field_name: str) -> str:
    if not isinstance(value, str):
        raise TypeError(f"{field_name} expects str")
    return value


def as_int(value: object, field_name: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool):
        raise TypeError(f"{field_name} expects int")
    return value


def as_bool(value: object, field_name: str) -> bool:
    if not isinstance(value, bool):
        raise TypeError(f"{field_name} expects bool")
    return value


def as_scalar(value: object, field_name: str) -> ScalarValue:
    if isinstance(value, str | bool | int | float):
        return value
    raise TypeError(f"{field_name} expects scalar")


def as_str_tuple(value: object, field_name: str) -> tuple[str, ...]:
    if not isinstance(value, Sequence) or isinstance(value, str | bytes | bytearray):
        raise TypeError(f"{field_name} expects sequence")
    return tuple(as_str(item, field_name) for item in value)


def sequence(value: object, field_name: str) -> Sequence[object]:
    if not isinstance(value, Sequence) or isinstance(value, str | bytes | bytearray):
        raise TypeError(f"{field_name} expects sequence")
    return value


def as_json_value(value: object) -> JsonValue:
    if value is None or isinstance(value, bool | int | float | str):
        return value
    if isinstance(value, Mapping):
        return {str(key): as_json_value(item) for key, item in value.items()}
    if isinstance(value, Sequence) and not isinstance(value, str | bytes | bytearray):
        return [as_json_value(item) for item in value]
    raise TypeError(f"value is not JSON serializable: {type(value).__qualname__}")


def as_json_dict(value: object, field_name: str) -> JsonDict:
    converted = as_json_value(value)
    if not isinstance(converted, dict):
        raise TypeError(f"{field_name} expects mapping")
    return converted


def as_scalar_dict(value: object, field_name: str) -> dict[str, ScalarValue]:
    if not isinstance(value, Mapping):
        raise TypeError(f"{field_name} expects mapping")
    return {str(key): as_scalar(item, field_name) for key, item in value.items()}


def optional_str(value: object, field_name: str) -> str | None:
    return None if value is None else as_str(value, field_name)


def optional_int(value: object, field_name: str) -> int | None:
    return None if value is None else as_int(value, field_name)


def required_optional[T](value: T | None, field_name: str) -> T:
    if value is None:
        raise TypeError(f"{field_name} is required for this variant")
    return value


def to_json_value(value: object) -> JsonValue:
    encoder = getattr(value, "to_json_value", None)
    if callable(encoder):
        return cast(JsonValue, encoder())
    if isinstance(value, StrEnum):
        return value.value
    if is_dataclass(value):
        return {item.name: to_json_value(getattr(value, item.name)) for item in fields(value)}
    if isinstance(value, Mapping):
        return {str(key): to_json_value(item) for key, item in value.items()}
    if isinstance(value, tuple | list):
        return [to_json_value(item) for item in value]
    if value is None or isinstance(value, bool | int | float | str):
        return value
    raise TypeError(f"value is not JSON serializable: {type(value).__qualname__}")


def to_json_dict(value: object) -> JsonDict:
    converted = to_json_value(value)
    if not isinstance(converted, dict):
        raise TypeError("top-level value must serialize to a JSON object")
    return converted


def to_json_bytes(value: object) -> bytes:
    return json.dumps(to_json_dict(value), separators=(",", ":"), ensure_ascii=False).encode()


def from_json_dict[T](contract_type: type[T], data: Mapping[str, object] | JsonDict) -> T:
    decoder = getattr(contract_type, "from_json_dict", None)
    if decoder is None:
        raise TypeError(f"{contract_type.__qualname__} does not expose from_json_dict")
    return cast(T, decoder(data))


def from_json_bytes[T](contract_type: type[T], data: bytes | bytearray | str) -> T:
    loaded: Any = json.loads(data)
    if not isinstance(loaded, Mapping):
        raise TypeError("top-level JSON value must be an object")
    return from_json_dict(contract_type, loaded)


def tuple_from_json[T](
    raw: Mapping[str, object], field_name: str, contract_type: type[T]
) -> tuple[T, ...]:
    return tuple(
        from_json_dict(contract_type, as_mapping(item, contract_type.__qualname__))
        for item in sequence(field_value(raw, field_name), field_name)
    )


def as_str_dict(raw: Mapping[str, object], field_name: str) -> dict[str, str]:
    value = field_value(raw, field_name)
    if not isinstance(value, Mapping):
        raise TypeError(f"{field_name} expects mapping")
    return {str(key): as_str(item, field_name) for key, item in value.items()}
