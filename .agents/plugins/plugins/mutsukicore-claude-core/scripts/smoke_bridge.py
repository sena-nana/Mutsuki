from __future__ import annotations

import subprocess
import sys
from pathlib import Path


def main() -> int:
    plugin_root = Path(__file__).resolve().parents[1]
    result = subprocess.run(
        [
            "cargo",
            "test",
            "--manifest-path",
            str(plugin_root / "Cargo.toml"),
            "smoke_runtime_routes_claude_agent",
        ],
        cwd=plugin_root,
        check=False,
    )
    return result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
