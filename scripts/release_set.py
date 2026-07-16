#!/usr/bin/env python3
"""Validate, synchronize, and report a Mutsuki release set."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Any

SHA_RE = re.compile(r"^[0-9a-f]{40}$")
GIT_DEP_RE = re.compile(
    r'git\s*=\s*"(?P<url>[^"]+)"(?P<body>[^}\n]*)rev\s*=\s*"(?P<rev>[0-9a-f]{40})"'
)
CORE_SOURCE_RE = re.compile(
    r"git\+https://github\.com/sena-nana/MutsukiCore\.git\?rev=([0-9a-f]{40})#([0-9a-f]{40})"
)
REQUIRED_REPOSITORIES = {
    "core",
    "service_host",
    "link",
    "std_plugins",
    "agent_kit",
    "bot_plugins",
    "distributed_host",
    "tauri_host",
    "python_runner_kit",
}


class ReleaseSetError(RuntimeError):
    pass


@dataclass(frozen=True)
class Repository:
    id: str
    url: str
    revision: str
    kind: str


@dataclass(frozen=True)
class ReleaseSet:
    path: Path
    release: str
    status: str
    runtime_wire_schema: str
    repositories: tuple[Repository, ...]
    raw: dict[str, Any]

    @property
    def by_id(self) -> dict[str, Repository]:
        return {repository.id: repository for repository in self.repositories}

    @property
    def by_url(self) -> dict[str, Repository]:
        return {normalize_url(repository.url): repository for repository in self.repositories}


def normalize_url(url: str) -> str:
    return url.removesuffix("/").removesuffix(".git").lower()


def load_release(path: Path) -> ReleaseSet:
    try:
        raw = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise ReleaseSetError(f"cannot read release manifest {path}: {error}") from error
    if raw.get("schema_version") != 1:
        raise ReleaseSetError("release manifest schema_version must be 1")
    repositories = tuple(Repository(**item) for item in raw.get("repositories", []))
    ids = [repository.id for repository in repositories]
    urls = [normalize_url(repository.url) for repository in repositories]
    if set(ids) != REQUIRED_REPOSITORIES or len(ids) != len(set(ids)):
        raise ReleaseSetError("release manifest must name every required repository exactly once")
    if len(urls) != len(set(urls)):
        raise ReleaseSetError("release manifest repository URLs must be unique")
    for repository in repositories:
        if not SHA_RE.fullmatch(repository.revision):
            raise ReleaseSetError(f"{repository.id} revision must be a full lowercase commit SHA")
        if repository.kind not in {"rust", "python"}:
            raise ReleaseSetError(f"{repository.id} has unsupported kind {repository.kind}")
    status = raw.get("status")
    if status not in {"active", "candidate", "unsupported"}:
        raise ReleaseSetError("release status must be active, candidate, or unsupported")
    if not raw.get("supported_deployments") or not raw.get("capabilities"):
        raise ReleaseSetError("release manifest must declare deployments and capability maturity")
    return ReleaseSet(
        path=path,
        release=str(raw.get("release", "")),
        status=status,
        runtime_wire_schema=str(raw.get("runtime_wire_schema", "")),
        repositories=repositories,
        raw=raw,
    )


def discover_active(releases_dir: Path) -> Path:
    active = [path for path in sorted(releases_dir.glob("*.toml")) if load_release(path).status == "active"]
    if len(active) != 1:
        raise ReleaseSetError(f"expected exactly one active release manifest, found {len(active)}")
    return active[0]


def git_dependencies(text: str) -> list[tuple[str, str]]:
    return [(normalize_url(match.group("url")), match.group("rev")) for match in GIT_DEP_RE.finditer(text)]


def validate_manifest_pins(release: ReleaseSet, manifest: Path) -> None:
    text = manifest.read_text(encoding="utf-8")
    for url, revision in git_dependencies(text):
        repository = release.by_url.get(url)
        if repository and revision != repository.revision:
            raise ReleaseSetError(
                f"{manifest} pins {repository.id} at {revision}, expected {repository.revision}"
            )


def validate_lock_core(release: ReleaseSet, lockfile: Path) -> None:
    text = lockfile.read_text(encoding="utf-8")
    revisions = {match.group(1) for match in CORE_SOURCE_RE.finditer(text)}
    expected = release.by_id["core"].revision
    if revisions != {expected}:
        raise ReleaseSetError(
            f"{lockfile} must resolve exactly Core {expected}; found {sorted(revisions)}"
        )


def validate_deployment_pins(release: ReleaseSet, deployments_dir: Path) -> None:
    expected = release.by_id["distributed_host"].revision
    for deployment in sorted(deployments_dir.glob("*.toml")):
        try:
            document = tomllib.loads(deployment.read_text(encoding="utf-8"))
        except (OSError, tomllib.TOMLDecodeError) as error:
            raise ReleaseSetError(f"cannot read deployment {deployment}: {error}") from error
        external_service = document.get("external_service")
        if not isinstance(external_service, dict):
            continue
        revision = external_service.get("revision")
        if revision != expected:
            raise ReleaseSetError(
                f"{deployment} pins distributed_host at {revision}, expected {expected}"
            )


def validate_active_set(release: ReleaseSet, root: Path) -> None:
    if release.status == "active" and discover_active(release.path.parent) != release.path:
        raise ReleaseSetError(f"{release.path} is not the unique active release")
    validate_manifest_pins(release, root / "Cargo.toml")
    validate_lock_core(release, root / "Cargo.lock")
    validate_deployment_pins(release, root / "deploy/distribution")


def repository_path(workspace_root: Path, repository: Repository) -> Path:
    name = repository.url.rstrip("/").rsplit("/", 1)[-1].removesuffix(".git")
    return workspace_root / name


def git(*args: str, cwd: Path) -> str:
    result = subprocess.run(
        ["git", *args], cwd=cwd, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE
    )
    if result.returncode:
        raise ReleaseSetError(result.stderr.strip() or f"git {' '.join(args)} failed in {cwd}")
    return result.stdout.strip()


def repository_file(repository: Path, revision: str, relative: str) -> str | None:
    result = subprocess.run(
        ["git", "show", f"{revision}:{relative}"],
        cwd=repository,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    return result.stdout if result.returncode == 0 else None


def repository_manifests(repository: Path, revision: str) -> list[str]:
    files = git("ls-tree", "-r", "--name-only", revision, cwd=repository).splitlines()
    return [path for path in files if path == "Cargo.toml" or path.endswith("/Cargo.toml")]


def build_report(release: ReleaseSet, workspace_root: Path) -> dict[str, Any]:
    results: list[dict[str, Any]] = []
    expected_core = release.by_id["core"].revision
    expected_wire = release.runtime_wire_schema
    for item in release.repositories:
        path = repository_path(workspace_root, item)
        entry: dict[str, Any] = {"id": item.id, "revision": item.revision, "ok": True, "errors": []}
        try:
            git("cat-file", "-e", f"{item.revision}^{{commit}}", cwd=path)
            for manifest_path in repository_manifests(path, item.revision):
                manifest = repository_file(path, item.revision, manifest_path) or ""
                for url, revision in git_dependencies(manifest):
                    owner = release.by_url.get(url)
                    if owner and revision != owner.revision:
                        entry["errors"].append(
                            f"{manifest_path} pins {owner.id} at {revision}, expected {owner.revision}"
                        )
            if item.id == "python_runner_kit":
                generated = repository_file(
                    path, item.revision, "src/mutsuki_runner_kit/wire/generated.py"
                ) or ""
                protocol = repository_file(
                    path, item.revision, "src/mutsuki_runner_kit/wire/protocol.py"
                ) or ""
                if expected_core not in generated:
                    entry["errors"].append("Python wire mirror does not name the release Core")
                if expected_wire not in protocol:
                    entry["errors"].append("Python wire schema does not match the release schema")
        except (OSError, ReleaseSetError) as error:
            entry["errors"].append(str(error))
        entry["ok"] = not entry["errors"]
        results.append(entry)
    return {
        "schema": "mutsuki.release.validation.v1",
        "release": release.release,
        "status": release.status,
        "ok": all(result["ok"] for result in results),
        "repositories": results,
    }


def sync_workspace(release: ReleaseSet, workspace_root: Path, update_locks: bool) -> list[Path]:
    changed_roots: set[Path] = set()
    for item in release.repositories:
        root = repository_path(workspace_root, item)
        if not root.is_dir():
            raise ReleaseSetError(f"missing checkout for {item.id}: {root}")
        for manifest in [root / "Cargo.toml", *root.glob("**/Cargo.toml")]:
            if not manifest.is_file() or "target" in manifest.parts:
                continue
            text = manifest.read_text(encoding="utf-8")

            def replace(match: re.Match[str]) -> str:
                owner = release.by_url.get(normalize_url(match.group("url")))
                if not owner or match.group("rev") == owner.revision:
                    return match.group(0)
                return match.group(0).replace(match.group("rev"), owner.revision)

            updated = GIT_DEP_RE.sub(replace, text)
            if updated != text:
                manifest.write_text(updated, encoding="utf-8")
                changed_roots.add(root)
    if update_locks:
        for root in sorted(changed_roots):
            if (root / "Cargo.toml").is_file():
                subprocess.run(["cargo", "update"], cwd=root, check=True)
    return sorted(changed_roots)


def materialize(release: ReleaseSet, workspace_root: Path) -> None:
    workspace_root.mkdir(parents=True, exist_ok=True)
    for repository in release.repositories:
        path = repository_path(workspace_root, repository)
        if path.exists():
            raise ReleaseSetError(f"materialize destination already exists: {path}")
        subprocess.run(
            ["git", "clone", "--filter=blob:none", "--no-checkout", repository.url, str(path)],
            check=True,
        )
        git("checkout", "--detach", repository.revision, cwd=path)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", type=Path)
    subparsers = parser.add_subparsers(dest="command", required=True)
    validate_parser = subparsers.add_parser("validate")
    validate_parser.add_argument("--root", type=Path, default=Path.cwd())
    report_parser = subparsers.add_parser("report")
    report_parser.add_argument("--workspace-root", type=Path, required=True)
    report_parser.add_argument("--output", type=Path)
    sync_parser = subparsers.add_parser("sync")
    sync_parser.add_argument("--workspace-root", type=Path, required=True)
    sync_parser.add_argument("--no-lock", action="store_true")
    materialize_parser = subparsers.add_parser("materialize")
    materialize_parser.add_argument("--workspace-root", type=Path, required=True)
    args = parser.parse_args(argv)
    root = getattr(args, "root", Path.cwd()).resolve()
    manifest = args.manifest or discover_active(root / "releases")
    release = load_release(manifest.resolve())
    if args.command == "validate":
        validate_active_set(release, root)
        print(f"validated {release.release} ({release.status})")
    elif args.command == "report":
        report = build_report(release, args.workspace_root.resolve())
        encoded = json.dumps(report, indent=2, sort_keys=True) + "\n"
        if args.output:
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(encoded, encoding="utf-8")
        print(encoded, end="")
        if not report["ok"]:
            return 1
    elif args.command == "sync":
        changed = sync_workspace(release, args.workspace_root.resolve(), not args.no_lock)
        print(json.dumps([str(path) for path in changed], indent=2))
    else:
        materialize(release, args.workspace_root.resolve())
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ReleaseSetError as error:
        print(f"release-set error: {error}", file=sys.stderr)
        raise SystemExit(2)
