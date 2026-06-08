"""Schema 兼容回调注册表 —— 默认 byte-equal + 可注册替换。

v0.1 阶段的 enforcement（codec / bus 层调用）尚未落地（v0.2 候选），
但本测试钉死 API 行为，避免下游迁移时 silently rot。
"""

from __future__ import annotations

from mutsukibot.contracts.schema import is_compatible, register_schema_compatibility


def test_default_compat_requires_byte_equal_versions() -> None:
    assert is_compatible("mutsukibot.test.unregistered", "1.0.0", "1.0.0")
    assert not is_compatible("mutsukibot.test.unregistered", "1.0.0", "1.0.1")


def test_register_callback_overrides_default() -> None:
    schema_id = "mutsukibot.test.compat-register"
    register_schema_compatibility(schema_id, lambda p, c: p.split(".")[0] == c.split(".")[0])
    assert is_compatible(schema_id, "1.4.0", "1.9.2")
    assert not is_compatible(schema_id, "1.0.0", "2.0.0")


def test_register_overwrites_previous_callback() -> None:
    schema_id = "mutsukibot.test.compat-overwrite"
    register_schema_compatibility(schema_id, lambda _p, _c: True)
    assert is_compatible(schema_id, "0.0.1", "9.9.9")
    register_schema_compatibility(schema_id, lambda _p, _c: False)
    assert not is_compatible(schema_id, "0.0.1", "0.0.1")
