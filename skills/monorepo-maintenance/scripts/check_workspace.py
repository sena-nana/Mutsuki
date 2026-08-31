#!/usr/bin/env python3
from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

if sys.version_info < (3, 11):
    raise SystemExit(
        "check_workspace.py requires Python 3.11+ for tomllib; "
        f"found {sys.version_info.major}.{sys.version_info.minor}"
    )

import tomllib  # noqa: E402  (imported after the version guard on purpose)


ROOT = Path(__file__).resolve().parents[3]
LEGACY_REPOSITORIES = (
    "MutsukiCore",
    "MutsukiLink",
    "MutsukiCliHost",
    "MutsukiServiceHost",
    "MutsukiTauriHost",
    "MutsukiWebHost",
    "MutsukiDistributedHost",
    "MutsukiAgentKit",
    "MutsukiBotPlugins",
    "MutsukiStdPlugins",
    "MutsukiPythonRunnerKit",
    "MutsukiBotTemplate",
)
REQUIRED_PATHS = (
    "AGENTS.md",
    "Cargo.toml",
    "Cargo.lock",
    "crates/link",
    "hosts/cli",
    "hosts/service",
    "hosts/tauri",
    "hosts/web",
    "hosts/distributed",
    "kits/agent",
    "kits/python-runner",
    "plugins/bot",
    "plugins/std",
    "products/bot",
    "docs/architecture/monorepo.md",
    "docs/architecture/package-boundaries.toml",
    "docs/architecture/refactor-behavior-matrix.md",
    "docs/decisions/0001-mutsuki-monorepo.md",
    "docs/migration/issue-44-ledger.md",
)
PACKAGE_BOUNDARIES = ROOT / "docs/architecture/package-boundaries.toml"

# `cargo fuzz` builds its crate standalone with instrumentation flags that must not reach the main
# build, so it is excluded from the root Workspace and therefore has to be its own workspace root.
# It ships no library used by the product; nothing else may take this exemption.
NESTED_WORKSPACE_EXCEPTIONS = {"fuzz/Cargo.toml"}

# Files allowed to opt out of the workspace `unsafe_code = "deny"` lint. Each entry names the
# single reason the exception exists. Adding a file here is a deliberate review decision:
# `unsafe` must be unavoidable at that boundary, and every block needs a `SAFETY:` argument.
UNSAFE_CODE_EXCEPTIONS = {
    # ABI v2 FFI boundary: raw pointers, manual Send/Sync and extern "C" entry points.
    "crates/mutsuki-plugin-api/src/lib.rs": "ABI v2 guest surface",
    "crates/mutsuki-plugin-host/src/lib.rs": "ABI v2 dynamic library loader",
    "crates/mutsuki-runtime-sdk/src/abi.rs": "ABI v2 SDK mirror",
    "crates/mutsuki-runtime-sdk/src/lib.rs": "RawWakerVTable noop waker",
    "plugins/bot/crates/mutsuki-plugin-catalog/src/execute.rs": "ABI entry symbol probe",
    # OS interfaces with no safe equivalent.
    "hosts/service/crates/mutsuki-service-runtime/src/process_metrics.rs": "libc RSS sampling",
    "hosts/service/crates/mutsuki-service-config/src/lib.rs": "env::set_var in secret tests",
    "plugins/std/plugins/mutsuki-plugin-resource-shared-memory/src/mapping.rs": "shared mapping",
    "plugins/std/plugins/mutsuki-plugin-resource-shared-memory/src/bin/mutsuki-shared-memory-child.rs": "shared mapping",
    # Measurement harnesses implementing GlobalAlloc or sampling process counters.
    "crates/mutsuki-runtime-benchmarks/src/allocator.rs": "allocation tracking",
    "crates/mutsuki-runtime-benchmarks/src/system_metrics.rs": "process counters",
    "crates/mutsuki-runtime-benchmarks/src/wire_p2/abi.rs": "ABI lane benchmark",
    "crates/mutsuki-runtime-host/examples/worker_pool_benchmark.rs": "process counters",
    "hosts/service/crates/mutsuki-service-benchmarks/src/bin/control_ipc_bench.rs": "allocation tracking",
    "kits/agent/crates/mutsuki-agent-benchmarks/src/measurement.rs": "allocation tracking",
    "plugins/bot/crates/mutsuki-bot-benchmarks/src/measurement.rs": "allocation tracking",
    "plugins/std/crates/mutsuki-std-benchmarks/src/main.rs": "allocation tracking",
    "plugins/std/crates/mutsuki-std-benchmarks/src/bin/effect_derive_bench.rs": "allocation tracking",
    "plugins/std/plugins/mutsuki-plugin-resource-shared-memory/tests/zero_copy.rs": "allocation budget test",
    "hosts/service/crates/mutsuki-service-runtime/examples/core_driver_benchmark.rs": "process counters",
    "hosts/service/crates/mutsuki-service-benchmarks/src/main.rs": "process counters",
    "hosts/tauri/crates/mutsuki-tauri-benchmarks/src/main.rs": "process counters",
    "hosts/tauri/crates/mutsuki-tauri-host/examples/task_pump_benchmark.rs": "process counters",
}
# Submodules of a file that already carries a module-level allow. They are listed separately so
# the exception stays enumerated per file rather than silently inherited.
UNSAFE_CODE_EXCEPTIONS.update(
    {
        "crates/mutsuki-runtime-sdk/src/abi/types.rs": "ABI v2 SDK mirror",
        "crates/mutsuki-runtime-sdk/src/abi/binary_host_client.rs": "ABI v2 SDK mirror",
        "crates/mutsuki-runtime-sdk/src/abi/tests.rs": "ABI v2 SDK mirror",
    }
)
UNSAFE_ALLOW_PATTERN = re.compile(r"#!?\[allow\(unsafe_code\)\]")
# Only constructs that actually introduce an unsafe obligation. `unsafe extern "C" fn(..)` in a
# type position and `#[unsafe(no_mangle)]` are deliberately excluded.
UNSAFE_USE_PATTERN = re.compile(r"unsafe\s*\{|unsafe\s+impl\b|^\s*(pub(\([^)]*\))?\s+)?unsafe\s+fn\b", re.M)
# Modules whose `unsafe` is covered by a module-level allow in their parent file.
UNSAFE_ALLOW_INHERITED = {
    "crates/mutsuki-runtime-sdk/src/abi/types.rs",
    "crates/mutsuki-runtime-sdk/src/abi/binary_host_client.rs",
    "crates/mutsuki-runtime-sdk/src/abi/tests.rs",
}


def fail(message: str) -> None:
    print(f"workspace boundary check failed: {message}", file=sys.stderr)
    raise SystemExit(1)


def cargo_metadata() -> dict[str, object]:
    result = subprocess.run(
        [
            "cargo",
            "metadata",
            "--locked",
            "--no-deps",
            "--format-version",
            "1",
        ],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode:
        fail(result.stderr.strip() or "cargo metadata failed")
    return json.loads(result.stdout)


def check_required_paths() -> None:
    missing = [path for path in REQUIRED_PATHS if not (ROOT / path).exists()]
    if missing:
        fail(f"missing required paths: {', '.join(missing)}")


def check_single_workspace() -> None:
    locks = sorted(
        path.relative_to(ROOT).as_posix()
        for path in ROOT.rglob("Cargo.lock")
        if ".git" not in path.parts and "target" not in path.parts
    )
    allowed = {"Cargo.lock", "fuzz/Cargo.lock"}
    unexpected = [path for path in locks if path not in allowed]
    if unexpected:
        fail(f"nested Cargo.lock files: {', '.join(unexpected)}")

    nested = []
    for manifest in ROOT.rglob("Cargo.toml"):
        relative = manifest.relative_to(ROOT).as_posix()
        if manifest == ROOT / "Cargo.toml" or "target" in manifest.parts:
            continue
        if relative in NESTED_WORKSPACE_EXCEPTIONS:
            continue
        with manifest.open("rb") as handle:
            if "workspace" in tomllib.load(handle):
                nested.append(relative)
    if nested:
        fail(f"nested Cargo workspaces: {', '.join(sorted(nested))}")


def check_integration_tests_index() -> None:
    """`integration-tests/` documents where acceptance tests live; it never holds tests itself.

    Cross-package tests belong beside the product or Host that owns the boundary, so they share its
    fixtures and are selected by the root Workspace. A test file landing here would be invisible to
    `cargo test --workspace`.
    """
    directory = ROOT / "integration-tests"
    if not directory.is_dir():
        fail("integration-tests/ is missing; it indexes the cross-package acceptance boundaries")
    strays = sorted(
        path.relative_to(ROOT).as_posix()
        for path in directory.rglob("*")
        if path.is_file() and path.name != "README.md"
    )
    if strays:
        fail(
            "integration-tests/ is an index, not a test crate; move these beside their owner:\n  "
            + "\n  ".join(strays)
        )


def check_metadata(metadata: dict[str, object]) -> None:
    workspace_root = Path(str(metadata["workspace_root"])).resolve()
    if workspace_root != ROOT:
        fail(f"cargo workspace root is {workspace_root}, expected {ROOT}")

    packages = metadata["packages"]
    names = [package["name"] for package in packages]
    if len(names) != len(set(names)):
        fail("package names are not unique")

    for package in packages:
        manifest = Path(package["manifest_path"]).resolve()
        if not manifest.is_relative_to(ROOT):
            fail(f"package is outside the repository: {manifest}")
        for dependency in package["dependencies"]:
            source = dependency.get("source") or ""
            if dependency["name"].startswith("mutsuki-") and source.startswith("git+"):
                fail(
                    f"{package['name']} uses Git for internal dependency "
                    f"{dependency['name']}: {source}"
                )


def _matches_source(rule: dict[str, object], package: dict[str, object]) -> bool:
    name = str(package["name"])
    manifest = Path(str(package["manifest_path"])).resolve()
    source = manifest.parent.relative_to(ROOT).as_posix()
    return any(
        (
            any(source == root or source.startswith(f"{root}/") for root in rule.get("source_roots", [])),
            name in rule.get("source_packages", []),
            any(name.startswith(prefix) for prefix in rule.get("source_package_prefixes", [])),
            any(name.endswith(suffix) for suffix in rule.get("source_package_suffixes", [])),
        )
    )


def check_package_boundaries(metadata: dict[str, object]) -> None:
    with PACKAGE_BOUNDARIES.open("rb") as handle:
        config = tomllib.load(handle)
    if config.get("version") != 1:
        fail("package-boundaries.toml must declare version = 1")

    violations: list[str] = []
    for rule in config.get("rules", []):
        rule_name = str(rule.get("name", "unnamed"))
        allowed_sources = set(rule.get("allow_source_packages", []))
        for package in metadata["packages"]:
            if package["name"] in allowed_sources or not _matches_source(rule, package):
                continue
            for dependency in package["dependencies"]:
                if dependency.get("kind") == "dev" or not dependency.get("path"):
                    continue
                target_path = Path(str(dependency["path"])).resolve()
                if not target_path.is_relative_to(ROOT):
                    continue
                target = target_path.relative_to(ROOT).as_posix()
                allowed_targets = set(rule.get("allow_target_packages", []))
                forbidden = (
                    dependency["name"] not in allowed_targets
                    and (
                        any(
                            target == root or target.startswith(f"{root}/")
                            for root in rule.get("forbidden_target_roots", [])
                        )
                        or dependency["name"] in rule.get("forbidden_target_packages", [])
                        or any(
                            dependency["name"].startswith(prefix)
                            for prefix in rule.get("forbidden_target_package_prefixes", [])
                        )
                    )
                )
                if forbidden:
                    violations.append(
                        f"{rule_name}: {package['name']} -> {dependency['name']} ({target})"
                    )
    if violations:
        fail("forbidden package dependencies:\n  " + "\n  ".join(sorted(violations)))


def check_workspace_lints(metadata: dict[str, object]) -> None:
    """Every member must inherit the root lint table.

    Without `[lints] workspace = true` a package silently opts out of `unsafe_code` and the
    clippy policy, so the gate passes while enforcing nothing.
    """
    missing: list[str] = []
    for package in metadata["packages"]:
        manifest = Path(str(package["manifest_path"])).resolve()
        with manifest.open("rb") as handle:
            lints = tomllib.load(handle).get("lints")
        if not isinstance(lints, dict) or lints.get("workspace") is not True:
            missing.append(
                f"{package['name']} ({manifest.parent.relative_to(ROOT).as_posix()})"
            )
    if missing:
        fail(
            "packages missing `[lints]\\nworkspace = true`:\n  "
            + "\n  ".join(sorted(missing))
        )


def check_unsafe_code_exceptions() -> None:
    """`unsafe` is confined to the reviewed exception list, and each block argues its safety."""
    unexpected_allow: list[str] = []
    unexpected_unsafe: list[str] = []
    undocumented: list[str] = []
    for path in ROOT.rglob("*.rs"):
        if "target" in path.parts or ".git" in path.parts:
            continue
        relative = path.relative_to(ROOT).as_posix()
        content = path.read_text(encoding="utf-8")
        allows = bool(UNSAFE_ALLOW_PATTERN.search(content)) or relative in UNSAFE_ALLOW_INHERITED
        uses = bool(UNSAFE_USE_PATTERN.search(content))
        if allows and relative not in UNSAFE_CODE_EXCEPTIONS:
            unexpected_allow.append(relative)
        elif uses and not allows:
            unexpected_unsafe.append(relative)
        if relative in UNSAFE_CODE_EXCEPTIONS and uses and "SAFETY:" not in content:
            undocumented.append(relative)

    problems: list[str] = []
    if unexpected_allow:
        problems.append(
            "files allow(unsafe_code) without being on the reviewed exception list:\n    "
            + "\n    ".join(sorted(unexpected_allow))
        )
    if unexpected_unsafe:
        problems.append(
            "files use `unsafe` without an exception entry:\n    "
            + "\n    ".join(sorted(unexpected_unsafe))
        )
    if undocumented:
        problems.append(
            "exception files without any `SAFETY:` argument:\n    "
            + "\n    ".join(sorted(undocumented))
        )
    stale = sorted(
        name
        for name in UNSAFE_CODE_EXCEPTIONS
        if not (ROOT / name).exists()
        or not (
            name in UNSAFE_ALLOW_INHERITED
            or UNSAFE_ALLOW_PATTERN.search((ROOT / name).read_text(encoding="utf-8"))
        )
    )
    if stale:
        problems.append(
            "exception entries that no longer need the allow (remove them):\n    "
            + "\n    ".join(stale)
        )
    if problems:
        fail("unsafe_code policy violated:\n  " + "\n  ".join(problems))


def check_manifest_urls() -> None:
    for manifest in ROOT.rglob("Cargo.toml"):
        if "target" in manifest.parts:
            continue
        content = manifest.read_text(encoding="utf-8")
        for repository in LEGACY_REPOSITORIES:
            legacy = f"github.com/sena-nana/{repository}.git"
            if legacy in content:
                fail(
                    f"legacy internal repository URL remains in "
                    f"{manifest.relative_to(ROOT)}: {legacy}"
                )


def main() -> None:
    check_required_paths()
    check_single_workspace()
    check_integration_tests_index()
    metadata = cargo_metadata()
    check_metadata(metadata)
    check_package_boundaries(metadata)
    check_workspace_lints(metadata)
    check_unsafe_code_exceptions()
    check_manifest_urls()
    print(
        "workspace boundary check passed: "
        f"{len(metadata['packages'])} Rust packages, one root workspace, no internal Git pins, "
        f"workspace lints inherited everywhere, "
        f"{len(UNSAFE_CODE_EXCEPTIONS)} reviewed unsafe_code exceptions"
    )


if __name__ == "__main__":
    main()
