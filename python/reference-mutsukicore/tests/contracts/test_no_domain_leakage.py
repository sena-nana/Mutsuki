"""核心必须保持领域中立。

按设计计划，核心代码路径里不允许出现领域专属词汇 ``latent`` / ``kv`` /
``tensor`` / ``gpu`` / ``vram``。这些属于领域契约包
（``mutsukicore-contracts-yume`` 等）。
"""

from __future__ import annotations

from pathlib import Path
import re

import pytest

CORE_PATH = Path(__file__).resolve().parents[2] / "mutsukicore" / "core"
RUST_RUNTIME_PATH = Path(__file__).resolve().parents[2] / "crates"
FORBIDDEN = ("latent", "tensor", "vram", "gpu")
RUST_FORBIDDEN = (
    "latent",
    "tensor",
    "vram",
    "gpu",
    "yume",
    "lilia",
    "codex",
    "onebot",
    "mcp",
    "chatcompletion",
)


@pytest.mark.parametrize("word", FORBIDDEN)
def test_core_has_no_domain_word(word: str) -> None:
    pattern = re.compile(rf"\b{word}\b", re.IGNORECASE)
    offenders: list[str] = []
    for py in CORE_PATH.rglob("*.py"):
        text = py.read_text(encoding="utf-8")
        # 是否容忍 docstring/注释里把这些词当例子？
        # 不 —— 规则是绝对的。即便例子也不能出现在 `core/`。
        if pattern.search(text):
            offenders.append(str(py.relative_to(CORE_PATH)))
    assert offenders == [], f"在 core 中发现禁止的领域词汇 {word!r}: {offenders}"


@pytest.mark.parametrize("word", RUST_FORBIDDEN)
def test_rust_runtime_core_has_no_domain_or_product_word(word: str) -> None:
    if not RUST_RUNTIME_PATH.exists():
        pytest.skip("Rust runtime crates not present")
    pattern = re.compile(rf"\b{word}\b", re.IGNORECASE)
    offenders: list[str] = []
    for path in RUST_RUNTIME_PATH.rglob("*"):
        if path.suffix not in {".rs", ".toml"}:
            continue
        text = path.read_text(encoding="utf-8")
        if pattern.search(text):
            offenders.append(str(path.relative_to(RUST_RUNTIME_PATH)))
    assert offenders == [], (
        f"在 Rust runtime crates 中发现禁止的领域/产品词汇 {word!r}: {offenders}"
    )
