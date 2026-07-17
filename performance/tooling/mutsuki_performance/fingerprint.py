from __future__ import annotations

import hashlib
import json
import os
import platform
import shutil
import subprocess
from typing import Any


def canonical_bytes(value: Any) -> bytes:
    return json.dumps(
        value, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode()


def canonical_sha256(value: Any) -> str:
    return hashlib.sha256(canonical_bytes(value)).hexdigest()


def _command(*argv: str) -> str:
    if not shutil.which(argv[0]):
        return "unavailable"
    result = subprocess.run(argv, capture_output=True, text=True, check=False)
    return (
        result.stdout.strip()
        if result.returncode == 0 and result.stdout.strip()
        else "unavailable"
    )


def _cpu_model() -> str:
    if platform.system() == "Darwin":
        return _command("sysctl", "-n", "machdep.cpu.brand_string")
    if platform.system() == "Windows":
        output = _command(
            "powershell",
            "-NoProfile",
            "-Command",
            "(Get-CimInstance Win32_Processor | Select-Object -First 1 -Expand Name)",
        )
        return output
    try:
        for line in open("/proc/cpuinfo", encoding="utf-8"):
            if line.lower().startswith("model name"):
                return line.split(":", 1)[1].strip()
    except OSError:
        pass
    return platform.processor() or "unavailable"


def _ram_bytes() -> int:
    if platform.system() == "Darwin":
        value = _command("sysctl", "-n", "hw.memsize")
        return int(value) if value.isdigit() else 1
    if platform.system() == "Windows":
        value = _command(
            "powershell",
            "-NoProfile",
            "-Command",
            "(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory",
        )
        return int(value) if value.isdigit() else 1
    if hasattr(os, "sysconf"):
        try:
            return int(os.sysconf("SC_PAGE_SIZE")) * int(os.sysconf("SC_PHYS_PAGES"))
        except (ValueError, OSError):
            pass
    return 1


def environment_fingerprint(
    *,
    target_triple: str = "unavailable",
    release_profile: dict[str, Any] | None = None,
    runner_configuration: dict[str, Any] | None = None,
    power_mode: str = "unrecorded",
    virtualization: str = "unrecorded",
) -> tuple[str, dict[str, Any]]:
    environment = {
        "cpu_model": _cpu_model(),
        "cpu_topology": f"logical={os.cpu_count() or 1}",
        "ram_bytes": _ram_bytes(),
        "os": f"{platform.system()} {platform.version()}",
        "kernel": platform.release(),
        "architecture": platform.machine(),
        "target_triple": target_triple,
        "toolchains": {
            "rust": _command("rustc", "--version"),
            "python": platform.python_version(),
            "node": _command("node", "--version"),
        },
        "release_profile": release_profile
        or {"name": "release", "lto": "unrecorded", "codegen_units": 1},
        "power_mode": power_mode,
        "virtualization": virtualization,
        "runner_configuration": runner_configuration or {},
    }
    return canonical_sha256(environment), environment
