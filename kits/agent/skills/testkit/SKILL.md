---
name: testkit
description: Add or change Agent conformance helpers, fake models, fake tools, fake memory, bundle tests, templates, examples, Runner tests, or end-to-end Agent task flows.
---

# Agent TestKit

- Exercise public protocol, SDK, service, Runner and bundle surfaces as consumers use them.
- Put reusable fakes in `mutsuki-agent-testkit`; never expose them as production fallback.
- Cover batch single/multi-entry, partial failure, task handles, trace/correlation and generation.
- Keep examples minimal and backed by real APIs; do not document unimplemented behavior.
- Verify model/tool/memory round trips and manifest conformance.

Run targeted package tests first, then workspace tests.
