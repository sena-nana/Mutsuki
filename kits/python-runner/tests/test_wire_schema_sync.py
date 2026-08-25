"""Guards the packaged wire artifacts against drift from their Rust source of truth.

Every other wire test in this suite reads the copies shipped inside
``mutsuki_runner_kit.wire``. That makes those tests self-consistent but structurally unable to
notice when the copies fall behind ``crates/mutsuki-runtime-wire/schema``: both sides simply
agree on a stale value. This module is the one place that compares the two trees byte for byte,
so a regenerated Rust schema fails Python CI instead of silently diverging.

When the kit is tested outside the monorepo (an installed wheel, for example) the Rust tree is
not on disk and these checks are skipped, with the pinned revision left to release tooling.
"""

from __future__ import annotations

from importlib.resources import files
from pathlib import Path

import pytest

WIRE_ARTIFACTS = (
    "runtime-wire-v1.json",
    "runtime-wire-fixtures-v1.json",
    "runtime-wire-binary-golden-v1.json",
)
RUST_SCHEMA_RELATIVE = Path("crates/mutsuki-runtime-wire/schema")


def _rust_schema_directory() -> Path | None:
    for parent in Path(__file__).resolve().parents:
        candidate = parent / RUST_SCHEMA_RELATIVE
        if candidate.is_dir():
            return candidate
    return None


@pytest.mark.parametrize("artifact", WIRE_ARTIFACTS)
def test_packaged_wire_artifact_matches_rust_source_of_truth(artifact: str) -> None:
    schema_directory = _rust_schema_directory()
    if schema_directory is None:
        pytest.skip("Rust wire schema is unavailable outside the monorepo checkout")

    expected = (schema_directory / artifact).read_bytes()
    actual = files("mutsuki_runner_kit.wire").joinpath(artifact).read_bytes()

    assert actual == expected, (
        f"{artifact} drifted from {RUST_SCHEMA_RELATIVE / artifact}. "
        "Regenerate it with `cargo run -p mutsuki-runtime-wire --bin export_runtime_wire -- "
        "write kits/python-runner/src/mutsuki_runner_kit/wire`."
    )


def test_every_rust_wire_artifact_is_mirrored_in_the_kit() -> None:
    schema_directory = _rust_schema_directory()
    if schema_directory is None:
        pytest.skip("Rust wire schema is unavailable outside the monorepo checkout")

    published = {path.name for path in schema_directory.glob("*.json")}

    assert published == set(WIRE_ARTIFACTS), (
        "the Rust wire schema directory gained or lost an artifact; mirror it into "
        "mutsuki_runner_kit.wire and list it in WIRE_ARTIFACTS"
    )
