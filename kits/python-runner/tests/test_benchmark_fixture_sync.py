"""Guards the packaged runner-fixture manifest against drift from the ServiceHost copy.

``test_benchmark_runners`` checks this kit's copy against this kit's runners, and the Rust
``executable_fixture_manifest_matches_builtin_behavior_and_hashes`` checks the ServiceHost copy
against the Rust runners. Both are self-consistent and neither reads the other tree, so editing
one copy leaves both suites green while the two languages benchmark different corpora -- which
defeats the point of a shared versioned corpus. This module is the one place that compares them,
mirroring what ``test_wire_schema_sync`` does for the wire artifacts.

When the kit is tested outside the monorepo (an installed wheel, for example) the ServiceHost
tree is not on disk and the check is skipped.
"""

from __future__ import annotations

from pathlib import Path

import pytest

FIXTURE_NAME = "runner-fixtures-v1.json"
KIT_RELATIVE = Path("benchmarks") / FIXTURE_NAME
SERVICE_RELATIVE = Path("hosts/service/fixtures/performance") / FIXTURE_NAME


def _service_fixture() -> Path | None:
    for parent in Path(__file__).resolve().parents:
        candidate = parent / SERVICE_RELATIVE
        if candidate.is_file():
            return candidate
    return None


def test_packaged_runner_fixture_matches_the_service_host_copy() -> None:
    service_fixture = _service_fixture()
    if service_fixture is None:
        pytest.skip("the ServiceHost fixture tree is unavailable outside the monorepo checkout")

    kit_fixture = Path(__file__).parents[1] / KIT_RELATIVE
    assert kit_fixture.is_file(), f"{KIT_RELATIVE} is missing from the kit"

    assert kit_fixture.read_bytes() == service_fixture.read_bytes(), (
        f"{KIT_RELATIVE} and {SERVICE_RELATIVE} diverged. They are one versioned corpus shared "
        "by the Python and Rust benchmark suites: copy whichever side you changed onto the "
        "other, then re-run both suites so each still matches its own runners."
    )
