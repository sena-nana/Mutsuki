---
name: conformance-testing
description: Add or change Python Runner Kit contract fixtures, fake runners, assertions, Rust-Python wire conformance, backend tests, transport tests, or packaging and independent-install validation.
---

# Conformance Testing

- Drive public contracts, backend and transport surfaces as a real runner process would.
- Cover batch-first single/multi-entry behavior, partial failure, cancellation, task handles and resource descriptors.
- Keep fakes inside testing helpers and never expose them as production fallback.
- Compare fixtures against a named, pushed MutsukiCore revision.
- Verify `uv sync --locked` and all checks without sibling repositories or editable local dependencies.

Run ruff, pyright and pytest and report the exact commands.
