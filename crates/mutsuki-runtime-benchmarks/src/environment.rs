use std::collections::BTreeMap;
use std::env;
use std::process::Command;

#[cfg(target_os = "linux")]
use std::fs;

use sha2::{Digest, Sha256};

use crate::report::{Environment, ReleaseProfile, RepositoryRevision};

pub fn capture() -> (String, Environment) {
    let environment = Environment {
        cpu_model: cpu_model(),
        cpu_topology: format!(
            "logical={}",
            std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(1)
        ),
        ram_bytes: ram_bytes(),
        os: format!("{} {}", env::consts::OS, command_output("uname", &["-v"])),
        kernel: command_output("uname", &["-r"]),
        architecture: env::consts::ARCH.into(),
        target_triple: rust_host_triple(),
        toolchains: BTreeMap::from([
            ("rust".into(), command_output("rustc", &["--version"])),
            ("python".into(), command_output("python3", &["--version"])),
            ("node".into(), command_output("node", &["--version"])),
        ]),
        release_profile: ReleaseProfile {
            name: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
            .into(),
            lto: "thin".into(),
            codegen_units: 1,
        },
        power_mode: env::var("MUTSUKI_BENCH_POWER_MODE").unwrap_or_else(|_| "unrecorded".into()),
        virtualization: env::var("MUTSUKI_BENCH_VIRTUALIZATION")
            .unwrap_or_else(|_| "unrecorded".into()),
        runner_configuration: BTreeMap::from([(
            "runner_instance_model".into(),
            "single-active-batch".into(),
        )]),
    };
    let encoded = canonical_json(&environment);
    (sha256_hex(&encoded), environment)
}

pub fn repository_revisions() -> BTreeMap<String, RepositoryRevision> {
    BTreeMap::from([(
        "MutsukiCore".into(),
        RepositoryRevision {
            revision: command_output("git", &["rev-parse", "HEAD"]),
            dirty: repository_is_dirty(),
            remote: command_output("git", &["remote", "get-url", "origin"]),
        },
    )])
}

fn repository_is_dirty() -> bool {
    Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .map_or(true, |output| {
            dirty_from_status(output.status.success(), &output.stdout)
        })
}

fn dirty_from_status(success: bool, stdout: &[u8]) -> bool {
    !success || !String::from_utf8_lossy(stdout).trim().is_empty()
}

pub fn revision_lock_hash(revisions: &BTreeMap<String, RepositoryRevision>) -> String {
    sha256_hex(&canonical_json(revisions))
}

pub fn generated_at() -> String {
    #[cfg(target_os = "windows")]
    {
        command_output(
            "powershell",
            &[
                "-NoProfile",
                "-Command",
                "(Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')",
            ],
        )
    }
    #[cfg(not(target_os = "windows"))]
    {
        command_output("date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"])
    }
}

pub fn command_output(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if stdout.is_empty() {
                String::from_utf8_lossy(&output.stderr).trim().to_string()
            } else {
                stdout
            }
        })
        .filter(|output| !output.is_empty())
        .unwrap_or_else(|| "unavailable".into())
}

fn rust_host_triple() -> String {
    command_output("rustc", &["-vV"])
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .unwrap_or("unavailable")
        .into()
}

fn cpu_model() -> String {
    #[cfg(target_os = "macos")]
    {
        return command_output("sysctl", &["-n", "machdep.cpu.brand_string"]);
    }
    #[cfg(target_os = "windows")]
    {
        return command_output(
            "powershell",
            &[
                "-NoProfile",
                "-Command",
                "Get-CimInstance Win32_Processor | Select-Object -First 1 -ExpandProperty Name",
            ],
        );
    }
    #[cfg(target_os = "linux")]
    {
        return fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|content| {
                content.lines().find_map(|line| {
                    line.strip_prefix("model name\t:")
                        .or_else(|| line.strip_prefix("model name :"))
                        .map(str::trim)
                        .map(str::to_string)
                })
            })
            .unwrap_or_else(|| "unavailable".into());
    }
    #[allow(unreachable_code)]
    "unavailable".into()
}

fn ram_bytes() -> u64 {
    #[cfg(target_os = "macos")]
    {
        return command_output("sysctl", &["-n", "hw.memsize"])
            .parse()
            .unwrap_or(1);
    }
    #[cfg(target_os = "windows")]
    {
        return command_output(
            "powershell",
            &[
                "-NoProfile",
                "-Command",
                "(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory",
            ],
        )
        .parse()
        .unwrap_or(1);
    }
    #[cfg(target_os = "linux")]
    {
        return fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|content| {
                content.lines().find_map(|line| {
                    line.strip_prefix("MemTotal:")
                        .and_then(|value| value.split_whitespace().next())
                        .and_then(|value| value.parse::<u64>().ok())
                        .map(|kibibytes| kibibytes * 1024)
                })
            })
            .unwrap_or(1);
    }
    #[allow(unreachable_code)]
    1
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn canonical_json(value: &impl serde::Serialize) -> Vec<u8> {
    serde_json::to_vec(&serde_json::to_value(value).expect("benchmark metadata must serialize"))
        .expect("canonical benchmark metadata must serialize")
}

#[cfg(test)]
mod tests {
    use super::dirty_from_status;

    #[test]
    fn repository_status_is_clean_only_for_successful_empty_output() {
        assert!(!dirty_from_status(true, b""));
        assert!(!dirty_from_status(true, b"\r\n"));
        assert!(dirty_from_status(true, b" M tracked.rs\n"));
        assert!(dirty_from_status(false, b""));
    }
}
