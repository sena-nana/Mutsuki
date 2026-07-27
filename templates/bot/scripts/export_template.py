#!/usr/bin/env python3
"""Export the canonical Bot template as an independent Cargo workspace."""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import sys
from pathlib import Path

DEPENDENCY_RE = re.compile(r"^([A-Za-z0-9_-]+)\s*=\s*(.+)$")
WORKSPACE_DEP_RE = re.compile(
    r"^([A-Za-z0-9_-]+)(?:\.workspace\s*=\s*true|\s*=\s*\{\s*workspace\s*=\s*true)"
)
REV_RE = re.compile(r"^[0-9a-f]{40}$")
TAG_RE = re.compile(r"^v[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?$")

EXCLUDED_NAMES = {
    ".codegraph",
    ".git",
    "Cargo.lock",
    "artifacts",
    "releases",
    "target",
}
EXCLUDED_FILES = {
    Path("docs/release-sets.md"),
    Path("scripts/release_set.py"),
    Path("scripts/test_release_set.py"),
}


class ExportError(RuntimeError):
    """Raised when an export would be incomplete or ambiguous."""


def workspace_dependencies(root_manifest: Path) -> dict[str, str]:
    dependencies: dict[str, str] = {}
    in_workspace_dependencies = False
    for line in root_manifest.read_text(encoding="utf-8").splitlines():
        if line == "[workspace.dependencies]":
            in_workspace_dependencies = True
            continue
        if in_workspace_dependencies and line.startswith("["):
            break
        if not in_workspace_dependencies:
            continue
        match = DEPENDENCY_RE.match(line)
        if match:
            dependencies[match.group(1)] = match.group(2)
    return dependencies


def template_workspace_dependencies(source: Path) -> set[str]:
    dependencies: set[str] = set()
    for manifest in sorted((source / "crates").glob("*/Cargo.toml")):
        for line in manifest.read_text(encoding="utf-8").splitlines():
            match = WORKSPACE_DEP_RE.match(line)
            if match and match.group(1) not in {"version", "edition", "license"}:
                dependencies.add(match.group(1))
    return dependencies


def render_manifest(
    source: Path,
    root_manifest: Path,
    repository: str,
    reference_kind: str,
    reference: str,
) -> str:
    root_dependencies = workspace_dependencies(root_manifest)
    required = template_workspace_dependencies(source)
    missing = sorted(required - root_dependencies.keys())
    if missing:
        raise ExportError(f"root workspace is missing template dependencies: {', '.join(missing)}")

    reference_key = "tag" if reference_kind == "tag" else "rev"
    lines = [
        "[workspace]",
        'members = ["crates/*"]',
        'resolver = "2"',
        "",
        "[workspace.package]",
        'version = "0.1.0"',
        'edition = "2024"',
        'rust-version = "1.88"',
        'license = "MIT"',
        "",
        "[workspace.dependencies]",
    ]
    for name in sorted(required):
        if name.startswith("mutsuki-"):
            value = f'{{ git = "{repository}", {reference_key} = "{reference}" }}'
        else:
            value = root_dependencies[name]
        lines.append(f"{name} = {value}")
    lines.extend(
        [
            "",
            "[profile.dev]",
            'debug = "line-tables-only"',
            "incremental = true",
            "",
        ]
    )
    return "\n".join(lines)


def resolve_revision(repository: str, reference_kind: str, reference: str) -> str:
    if reference_kind == "rev":
        return reference
    result = subprocess.run(
        ["git", "ls-remote", repository, f"refs/tags/{reference}", f"refs/tags/{reference}^{{}}"],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if result.returncode:
        raise ExportError(result.stderr.strip() or f"cannot resolve tag {reference}")
    revisions = [line.split()[0] for line in result.stdout.splitlines() if line.strip()]
    if not revisions:
        raise ExportError(f"tag does not exist in repository: {reference}")
    return revisions[-1]


def render_release_manifest(repository: str, reference: str, revision: str) -> str:
    return "\n".join(
        [
            "schema_version = 1",
            f'release = "{reference}"',
            'status = "active"',
            'contracts_api = "0.1.0"',
            'runtime_wire_schema = "mutsuki.runtime.wire/1.3.0"',
            'supported_deployments = ["service", "desktop", "web", "python-runner"]',
            'unsupported_deployments = ["distributed-clustered-production"]',
            "",
            "[[repositories]]",
            'id = "mutsuki"',
            f'url = "{repository}"',
            f'revision = "{revision}"',
            'kind = "monorepo"',
            "",
        ]
    )


def materialize_workspace_revisions(output: Path, revision: str) -> None:
    changed = 0
    for deployment in sorted((output / "deploy" / "distribution").glob("*.toml")):
        source = deployment.read_text(encoding="utf-8")
        rendered = source.replace('revision = "workspace"', f'revision = "{revision}"')
        if rendered != source:
            deployment.write_text(rendered, encoding="utf-8")
            changed += 1
    if changed == 0:
        raise ExportError("no canonical distribution deployment revision placeholders were found")


def validate_reference(reference_kind: str, reference: str) -> None:
    if reference_kind == "rev" and not REV_RE.fullmatch(reference):
        raise ExportError("--rev must be a full lowercase 40-character commit SHA")
    if reference_kind == "tag" and not TAG_RE.fullmatch(reference):
        raise ExportError("--tag must be a semantic release tag such as v0.1.0")


def ignored_paths(source: Path):
    def ignore(directory: str, names: list[str]) -> set[str]:
        base = Path(directory)
        ignored = {name for name in names if name in EXCLUDED_NAMES}
        for name in names:
            candidate = (base / name).relative_to(source)
            if candidate in EXCLUDED_FILES:
                ignored.add(name)
        return ignored

    return ignore


def export_template(
    source: Path,
    root_manifest: Path,
    output: Path,
    repository: str,
    reference_kind: str,
    reference: str,
) -> None:
    validate_reference(reference_kind, reference)
    if output.exists() and any(output.iterdir()):
        raise ExportError(f"output directory is not empty: {output}")
    output.parent.mkdir(parents=True, exist_ok=True)
    shutil.copytree(
        source,
        output,
        dirs_exist_ok=True,
        ignore=ignored_paths(source),
    )
    manifest = render_manifest(source, root_manifest, repository, reference_kind, reference)
    (output / "Cargo.toml").write_text(manifest, encoding="utf-8")
    revision = resolve_revision(repository, reference_kind, reference)
    materialize_workspace_revisions(output, revision)
    release = render_release_manifest(repository, reference, revision)
    (output / "release.toml").write_text(release, encoding="utf-8")
    result = subprocess.run(
        ["cargo", "generate-lockfile"],
        cwd=output,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if result.returncode:
        raise ExportError(result.stderr.strip() or "cargo generate-lockfile failed")


def main(argv: list[str] | None = None) -> int:
    script = Path(__file__).resolve()
    source = script.parents[1]
    monorepo = script.parents[3]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, default=source)
    parser.add_argument("--root-manifest", type=Path, default=monorepo / "Cargo.toml")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--repository",
        default="https://github.com/sena-nana/Mutsuki.git",
        help="Git repository used for every Mutsuki package",
    )
    reference = parser.add_mutually_exclusive_group(required=True)
    reference.add_argument("--tag")
    reference.add_argument("--rev")
    args = parser.parse_args(argv)
    kind = "tag" if args.tag else "rev"
    value = args.tag or args.rev
    assert value is not None
    export_template(
        args.source.resolve(),
        args.root_manifest.resolve(),
        args.output.resolve(),
        args.repository,
        kind,
        value,
    )
    print(f"exported template to {args.output.resolve()} at {kind} {value}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ExportError as error:
        print(f"template export error: {error}", file=sys.stderr)
        raise SystemExit(2)
