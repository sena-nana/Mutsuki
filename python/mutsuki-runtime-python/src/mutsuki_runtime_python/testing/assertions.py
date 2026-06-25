from __future__ import annotations

from mutsuki_runtime_python.contracts.codec import from_json_dict, to_json_dict


def assert_json_roundtrip[T](contract_type: type[T], value: T) -> T:
    encoded = to_json_dict(value)
    decoded = from_json_dict(contract_type, encoded)
    assert decoded == value
    return decoded
