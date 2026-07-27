from __future__ import annotations

import hashlib
import json
from dataclasses import replace
from pathlib import Path

import pytest

from mutsuki_runner_kit.contracts.task import Task
from mutsuki_runner_kit.runners.backend import PythonRunnerBackend
from mutsuki_runner_kit.testing.batches import multi_entry_batch, runner_context
from mutsuki_runner_kit.testing.benchmark_runners import (
    calibrated_checksum,
    fixture_output,
    standard_fixture_runners,
)


@pytest.mark.asyncio
async def test_standard_benchmark_fixtures_match_versioned_rust_outputs() -> None:
    manifest = json.loads(
        (Path(__file__).parents[1] / "benchmarks/runner-fixtures-v1.json").read_text()
    )
    fixtures = {item["protocol_id"]: item for item in manifest["fixtures"]}
    runners = standard_fixture_runners()
    assert {runner.protocol_id for runner in runners} == set(fixtures)

    for runner in runners:
        fixture = fixtures[runner.protocol_id]
        expected = fixture_output(runner.protocol_id, fixture["payload"])
        encoded = json.dumps(expected, sort_keys=True, separators=(",", ":")).encode()
        assert expected == fixture["output"]
        assert hashlib.sha256(encoded).hexdigest() == fixture["output_sha256"]

        backend = PythonRunnerBackend()
        backend.register_runner(runner)
        lease_id = f"lease-{runner.protocol_id}"
        task = replace(
            Task.new(f"task-{runner.protocol_id}", runner.protocol_id, fixture["payload"]),
            lease_id=lease_id,
        )
        batch = multi_entry_batch((task,), lease_ids=(lease_id,))
        ctx = runner_context(lease_ids=(lease_id,), batch_id=batch.batch_id)
        completion = await backend.run_batch_runner(runner.descriptor.runner_id, ctx, batch)
        result = completion.results[0]
        if runner.protocol_id == "runner.fault":
            assert result.result is None
            assert result.error is not None
            assert result.error.code == "fixture.failure"
        else:
            assert result.error is None
            assert result.result is not None
            assert result.result.output == expected


def test_calibrated_checksum_matches_rust_wrapping_fixture() -> None:
    assert calibrated_checksum(1_297_435_713, 4_096) == "8daeb4710bda7ecb"
