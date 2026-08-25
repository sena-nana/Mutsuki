"""Keeps the Python error-code mirror aligned with `mutsuki-runtime-contracts`.

The error codes are a closed set: a runner that receives a code it does not know cannot make a
correct recovery decision. Mirroring them by hand is exactly the kind of drift that stays
invisible until a plugin misclassifies a failure, so this test reads the Rust declarations
directly rather than trusting a second hand-written list.

When the kit is tested outside the monorepo the Rust tree is absent and the check is skipped.
"""

from __future__ import annotations

import re
from pathlib import Path

import pytest

from mutsuki_runner_kit.contracts import errors

RUST_CONTRACTS_RELATIVE = Path("crates/mutsuki-runtime-contracts/src")
RUST_ERROR_CONST = re.compile(
    r'pub const (ERR_[A-Z0-9_]+): &str = "([^"]+)"', re.MULTILINE
)


def _rust_contracts_directory() -> Path | None:
    for parent in Path(__file__).resolve().parents:
        candidate = parent / RUST_CONTRACTS_RELATIVE
        if candidate.is_dir():
            return candidate
    return None


def _rust_error_codes(directory: Path) -> dict[str, str]:
    codes: dict[str, str] = {}
    for source in sorted(directory.rglob("*.rs")):
        for name, value in RUST_ERROR_CONST.findall(source.read_text(encoding="utf-8")):
            codes[name] = value
    return codes


def _python_error_codes() -> dict[str, str]:
    return {
        name: value
        for name, value in vars(errors).items()
        if name.startswith("ERR_") and isinstance(value, str)
    }


def test_python_mirrors_every_core_error_code_with_the_same_value() -> None:
    directory = _rust_contracts_directory()
    if directory is None:
        pytest.skip("Rust contracts are unavailable outside the monorepo checkout")

    rust = _rust_error_codes(directory)
    python = _python_error_codes()

    assert rust, "no error-code constants found in mutsuki-runtime-contracts"
    assert python == rust, (
        "the Python error-code mirror diverged from mutsuki-runtime-contracts.\n"
        f"missing in Python: {sorted(set(rust) - set(python))}\n"
        f"unknown to Core:   {sorted(set(python) - set(rust))}\n"
        f"different values:  "
        f"{sorted(name for name in set(rust) & set(python) if rust[name] != python[name])}"
    )
