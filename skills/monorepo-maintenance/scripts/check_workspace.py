#!/usr/bin/env python3
from __future__ import annotations

import json
import subprocess
import sys
import tomllib
from pathlib import Path


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
        if manifest == ROOT / "Cargo.toml" or "target" in manifest.parts:
            continue
        with manifest.open("rb") as handle:
            if "workspace" in tomllib.load(handle):
                nested.append(manifest.relative_to(ROOT).as_posix())
    if nested:
        fail(f"nested Cargo workspaces: {', '.join(sorted(nested))}")


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
                forbidden = any(
                    target == root or target.startswith(f"{root}/")
                    for root in rule.get("forbidden_target_roots", [])
                ) or dependency["name"] in rule.get("forbidden_target_packages", [])
                if forbidden:
                    violations.append(
                        f"{rule_name}: {package['name']} -> {dependency['name']} ({target})"
                    )
    if violations:
        fail("forbidden package dependencies:\n  " + "\n  ".join(sorted(violations)))


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
    metadata = cargo_metadata()
    check_metadata(metadata)
    check_package_boundaries(metadata)
    check_manifest_urls()
    print(
        "workspace boundary check passed: "
        f"{len(metadata['packages'])} Rust packages, one root workspace, no internal Git pins"
    )


if __name__ == "__main__":
    main()
