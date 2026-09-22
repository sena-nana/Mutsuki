#!/usr/bin/env python3
"""Check the independent Rust gates in the repository's block-style CI YAML.

This deliberately reads only top-level job/step scalar fields, not shell bodies
or arbitrary YAML. Unsupported condition syntax fails closed. No YAML package
is needed by the Python 3.11 workspace gate.
"""
from __future__ import annotations

import re
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[4]
GATES = ("Lint", "Performance smoke gate", "Check fuzz targets")


def rust_steps(workflow: str) -> dict[str, dict[str, str]]:
    job = re.search(r"^  rust:\s*\n(.*?)(?=^  \S|\Z)", workflow, re.M | re.S)
    if job is None:
        raise ValueError("CI must contain the rust job")
    steps: dict[str, dict[str, str]] = {}
    for block in re.split(r"^      - ", job[1], flags=re.M)[1:]:
        fields: dict[str, str] = {}
        for key, value in re.findall(r"^(?:        )?([\w-]+):[ \t]*(.*)$", block, re.M):
            if key in fields:
                raise ValueError(f"duplicate step field: {key}")
            fields[key] = value.strip().strip("\"'")
        name = fields.get("name", "")
        if name in steps:
            raise ValueError(f"duplicate step name: {name}")
        steps[name] = fields
    return steps


def will_run(condition: str | None, *, failed: bool, cancelled: bool) -> bool:
    expression = re.sub(r"\s+", "", condition or "success()")
    if expression.startswith("${{") and expression.endswith("}}"):
        expression = expression[3:-2]
    # These are GitHub's status-check conditions; an omitted if means success().
    values = {
        "success()": not failed and not cancelled,
        "failure()": failed,
        "always()": True,
        "!cancelled()": not cancelled,
    }
    if expression not in values:
        raise ValueError(f"unsupported independent-gate condition: {condition}")
    return values[expression]


def check_workflow(workflow: str) -> None:
    steps = rust_steps(workflow)
    for name in GATES:
        if name not in steps or not steps[name].get("run"):
            raise ValueError(f"missing executable CI gate: {name}")
        if steps[name].get("continue-on-error", "false") != "false":
            raise ValueError(f"CI gate must propagate failure: {name}")
        for failed in (False, True):
            for cancelled in (False, True):
                if will_run(steps[name].get("if"), failed=failed, cancelled=cancelled) != (not cancelled):
                    raise ValueError(f"{name} must run after success/failure, but stop on cancellation")


class CiGateTests(unittest.TestCase):
    def setUp(self) -> None:
        self.workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")

    def test_repository_gates_run_after_failure_and_stop_on_cancellation(self) -> None:
        check_workflow(self.workflow)

    def test_regressed_conditions_are_rejected_for_each_gate(self) -> None:
        for name in GATES:
            for condition in (None, "${{ success() }}", "${{ failure() }}", "${{ always() }}"):
                with self.subTest(gate=name, condition=condition):
                    original = rust_steps(self.workflow)[name]["if"]
                    start = self.workflow.index(f"      - name: {name}\n")
                    prefix, tail = self.workflow[:start], self.workflow[start:]
                    line = f"        if: {original}\n"
                    replacement = "" if condition is None else f"        if: {condition}\n"
                    changed = prefix + tail.replace(line, replacement, 1)
                    self.assertNotEqual(changed, self.workflow)
                    with self.assertRaises(ValueError):
                        check_workflow(changed)

    def test_failure_cannot_be_hidden(self) -> None:
        for name in GATES:
            with self.subTest(gate=name):
                changed = self.workflow.replace(
                    f"      - name: {name}\n",
                    f"      - name: {name}\n        continue-on-error: true\n",
                    1,
                )
                with self.assertRaises(ValueError):
                    check_workflow(changed)

    def test_shell_text_cannot_supply_a_missing_condition(self) -> None:
        changed = self.workflow.replace("        if: ${{ !cancelled() }}\n", "")
        changed += "\n          echo 'if: ${{ !cancelled() }}'\n"
        with self.assertRaises(ValueError):
            check_workflow(changed)


if __name__ == "__main__":
    unittest.main()
