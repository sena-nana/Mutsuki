#!/usr/bin/env python3
"""Transfer every retained Issue #44 source issue into the Mutsuki tracker."""

from __future__ import annotations

import argparse
import json
import subprocess
import time
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

OWNER = "sena-nana"
TARGET = "Mutsuki"


@dataclass(frozen=True)
class Source:
    repository: str
    area: str


SOURCES = (
    Source("MutsukiLink", "link"),
    Source("MutsukiCliHost", "cli"),
    Source("MutsukiServiceHost", "service"),
    Source("MutsukiTauriHost", "tauri"),
    Source("MutsukiWebHost", "web"),
    Source("MutsukiDistributedHost", "distributed"),
    Source("MutsukiAgentKit", "agent"),
    Source("MutsukiBotPlugins", "bot"),
    Source("MutsukiStdPlugins", "std"),
    Source("MutsukiPythonRunnerKit", "python-runner"),
    Source("MutsukiBotTemplate", "bot-template"),
)

PRETRANSFERRED = (
    {
        "source_repository": f"{OWNER}/MutsukiLink",
        "source_number": 1,
        "source_url": f"https://github.com/{OWNER}/MutsukiLink/issues/1",
        "target_number": 45,
        "area": "link",
    },
)


def gh_json(*arguments: str, stdin: dict[str, Any] | None = None) -> Any:
    command = ["gh", *arguments]
    result = subprocess.run(
        command,
        input=None if stdin is None else json.dumps(stdin),
        text=True,
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"{' '.join(command)} failed ({result.returncode}): {result.stderr.strip()}"
        )
    return json.loads(result.stdout) if result.stdout.strip() else None


def ensure_label(name: str, color: str, description: str) -> None:
    result = subprocess.run(
        [
            "gh",
            "api",
            "--method",
            "POST",
            f"repos/{OWNER}/{TARGET}/labels",
            "--input",
            "-",
        ],
        input=json.dumps({"name": name, "color": color, "description": description}),
        text=True,
        capture_output=True,
        check=False,
    )
    if result.returncode != 0 and "HTTP 422" not in result.stderr:
        raise RuntimeError(f"cannot create label {name}: {result.stderr.strip()}")


def list_issues(repository: str) -> list[dict[str, Any]]:
    issues = gh_json(
        "issue",
        "list",
        "--repo",
        f"{OWNER}/{repository}",
        "--state",
        "all",
        "--limit",
        "1000",
        "--json",
        "number,title,state,url",
    )
    return sorted(issues, key=lambda issue: issue["number"])


def add_labels(issue_number: int, labels: list[str]) -> None:
    endpoint = f"repos/{OWNER}/{TARGET}/issues/{issue_number}/labels"
    error: Exception | None = None
    for attempt in range(12):
        try:
            gh_json(
                "api",
                "--method",
                "POST",
                endpoint,
                "--input",
                "-",
                stdin={"labels": labels},
            )
            return
        except RuntimeError as caught:
            error = caught
            time.sleep(min(0.5 * (attempt + 1), 3.0))
    raise RuntimeError(f"cannot label transferred issue {issue_number}: {error}")


def issue_details(issue_number: int) -> dict[str, Any]:
    return gh_json(
        "issue",
        "view",
        str(issue_number),
        "--repo",
        f"{OWNER}/{TARGET}",
        "--json",
        "number,state,title,url",
    )


def transfer_issue(source: Source, issue_number: int) -> dict[str, Any]:
    command = [
        "gh",
        "issue",
        "transfer",
        str(issue_number),
        f"{OWNER}/{TARGET}",
        "--repo",
        f"{OWNER}/{source.repository}",
    ]
    result = subprocess.run(command, text=True, capture_output=True, check=False)
    if result.returncode != 0:
        raise RuntimeError(
            f"{' '.join(command)} failed ({result.returncode}): {result.stderr.strip()}"
        )
    target_number = int(result.stdout.strip().rsplit("/", 1)[1])
    return issue_details(target_number)


def write_mapping(path: Path, entries: list[dict[str, Any]]) -> None:
    payload = {
        "schema": "mutsuki.issue-44.issue-map/v1",
        "target_repository": f"{OWNER}/{TARGET}",
        "generated_at": datetime.now(UTC).isoformat(),
        "issues": entries,
    }
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n")


def load_mapping(path: Path) -> list[dict[str, Any]]:
    if not path.exists():
        return []
    payload = json.loads(path.read_text())
    if payload.get("schema") != "mutsuki.issue-44.issue-map/v1":
        raise RuntimeError(f"unexpected mapping schema in {path}")
    return payload["issues"]


def wait_until_source_empty(repository: str) -> None:
    remaining: list[dict[str, Any]] = []
    for attempt in range(12):
        remaining = list_issues(repository)
        if not remaining:
            return
        time.sleep(min(0.5 * (attempt + 1), 3.0))
    raise RuntimeError(f"{repository} still has {len(remaining)} issues after transfer")


def transfer_all(output: Path) -> None:
    ensure_label(
        "migration:issue-44",
        "5319e7",
        "Transferred during the Issue #44 monorepo migration",
    )
    for source in SOURCES:
        ensure_label(
            f"area:{source.area}",
            "1d76db",
            f"Owned by the {source.area} area of the Mutsuki monorepo",
        )

    entries = load_mapping(output)
    recorded = {
        (entry["source_repository"], entry["source_number"]) for entry in entries
    }
    for previous in PRETRANSFERRED:
        key = (previous["source_repository"], previous["source_number"])
        if key in recorded:
            continue
        transferred = issue_details(previous["target_number"])
        add_labels(
            previous["target_number"],
            ["migration:issue-44", f"area:{previous['area']}"],
        )
        entries.append(
            {
                "source_repository": previous["source_repository"],
                "source_number": previous["source_number"],
                "source_url": previous["source_url"],
                "target_repository": f"{OWNER}/{TARGET}",
                "target_number": transferred["number"],
                "target_url": transferred["url"],
                "state": transferred["state"].lower(),
                "title": transferred["title"],
            }
        )
        recorded.add(key)
    write_mapping(output, entries)

    for source in SOURCES:
        issues = list_issues(source.repository)
        print(f"{source.repository}: transferring {len(issues)} issues", flush=True)
        for issue in issues:
            key = (f"{OWNER}/{source.repository}", issue["number"])
            if key in recorded:
                continue
            transferred = transfer_issue(source, issue["number"])
            target_number = transferred["number"]
            add_labels(
                target_number,
                ["migration:issue-44", f"area:{source.area}"],
            )
            entries.append(
                {
                    "source_repository": f"{OWNER}/{source.repository}",
                    "source_number": issue["number"],
                    "source_url": issue["url"],
                    "target_repository": f"{OWNER}/{TARGET}",
                    "target_number": target_number,
                    "target_url": transferred["url"],
                    "state": transferred["state"].lower(),
                    "title": transferred["title"],
                }
            )
            recorded.add(key)
            write_mapping(output, entries)
            print(
                f"  #{issue['number']} -> {OWNER}/{TARGET}#{target_number}",
                flush=True,
            )

        wait_until_source_empty(source.repository)

    if len(entries) != 117:
        raise RuntimeError(f"expected 117 transferred issues, got {len(entries)}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--output",
        type=Path,
        default=Path("docs/migration/issue-44-issue-map.json"),
    )
    args = parser.parse_args()
    transfer_all(args.output.resolve())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
