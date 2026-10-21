use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ScalarValue;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuntimeError {
    pub code: String,
    pub source: String,
    pub route: String,
    pub lost_capability: Option<String>,
    pub recovery: Option<String>,
    pub cause: Option<Box<RuntimeError>>,
    pub evidence: BTreeMap<String, ScalarValue>,
}

impl RuntimeError {
    pub fn new(
        code: impl Into<String>,
        source: impl Into<String>,
        route: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            source: source.into(),
            route: route.into(),
            lost_capability: None,
            recovery: None,
            cause: None,
            evidence: BTreeMap::new(),
        }
    }
}

pub const ERR_PLUGIN_DISABLED: &str = "plugin.disabled";
pub const ERR_PLUGIN_NOT_FOUND: &str = "plugin.not_found";
pub const ERR_RUNTIME_HOST_FAILED: &str = "runtime.host_failed";
pub const ERR_RUNTIME_HOST_GENERATION_MISMATCH: &str = "runtime.host_generation_mismatch";
pub const ERR_RUNTIME_NOT_ACCEPTING: &str = "runtime.not_accepting";
pub const ERR_RUNTIME_ABORTED: &str = "runtime.aborted";
pub const ERR_PORTABLE_SCHEMA_UNSUPPORTED: &str = "portable.schema_unsupported";
pub const ERR_CHECKPOINT_INCOMPATIBLE: &str = "checkpoint.incompatible";
pub const ERR_EXECUTION_NO_VARIANT: &str = "execution.no_variant";
pub const ERR_CAPABILITY_EXHAUSTED: &str = "capability.exhausted";
pub const ERR_TASK_NOT_FOUND: &str = "task.not_found";
pub const ERR_TASK_DUPLICATE: &str = "task.duplicate";
pub const ERR_TASK_CLAIM_CONFLICT: &str = "task.claim_conflict";
pub const ERR_TASK_EXPIRED: &str = "task.expired";
pub const ERR_TASK_DEAD_LETTER: &str = "task.dead_letter";
pub const ERR_TASK_UNSUPPORTED: &str = "task.unsupported";
pub const ERR_TASK_CANCEL_POLICY_UNSUPPORTED: &str = "task.cancel_policy_unsupported";
pub const ERR_TASK_SELF_CALL_BLOCKED: &str = "task.self_call_blocked";
pub const ERR_RUNNER_NOT_FOUND: &str = "runner.not_found";
pub const ERR_RUNNER_PURITY_VIOLATION: &str = "runner.purity_violation";
pub const ERR_RUNNER_AWAITABLE_UNSUPPORTED: &str = "runner.awaitable_unsupported";
pub const ERR_REGISTRY_FROZEN: &str = "registry.frozen";
pub const ERR_REGISTRY_UNAUTHORIZED: &str = "registry.unauthorized";
pub const ERR_REGISTRY_GENERATION_MISMATCH: &str = "registry.generation_mismatch";
pub const ERR_STATE_CONFLICT: &str = "state.conflict";
pub const ERR_STATE_HISTORY_DISABLED: &str = "state.history_disabled";
pub const ERR_STATE_HISTORY_UNAVAILABLE: &str = "state.history_unavailable";
pub const ERR_RESOURCE_NOT_FOUND: &str = "resource.not_found";
pub const ERR_RESOURCE_UNSUPPORTED: &str = "resource.unsupported";
pub const ERR_RESOURCE_LEASE_EXPIRED: &str = "resource.lease_expired";
pub const ERR_RESOURCE_LEASE_CROSS_AWAIT: &str = "resource.lease_cross_await";
pub const ERR_RESOURCE_GENERATION_MISMATCH: &str = "resource.generation_mismatch";
pub const ERR_RESOURCE_ENCODE_FAILED: &str = "resource.encode_failed";
pub const ERR_STATE_DELTA_DECODE_FAILED: &str = "state.delta_decode_failed";
pub const ERR_EVENT_DECODE_FAILED: &str = "event.decode_failed";
pub const ERR_RUNNER_FAILED: &str = "runner.failed";
pub const ERR_SDK_DECODE_FAILED: &str = "sdk.decode_failed";
pub const ERR_RELOAD_BLOCKED: &str = "plugin.reload_blocked";
pub const ERR_PLUGIN_CAPABILITY_UNAVAILABLE: &str = "plugin.capability_unavailable";
pub const ERR_RESOURCE_KIND_MISMATCH: &str = "resource.kind_mismatch";
pub const ERR_RESOURCE_LEASE_MODE_INVALID: &str = "resource.lease_mode_invalid";
pub const ERR_RESOURCE_SEMANTIC_MISMATCH: &str = "resource.semantic_mismatch";
pub const ERR_RUNTIME_TICK_INVALID: &str = "runtime.tick.invalid";
pub const ERR_SDK_SERIALIZE_FAILED: &str = "sdk.serialize_failed";
pub const ERR_STATE_ROLLBACK_DECODE_FAILED: &str = "state.rollback_decode_failed";
pub const ERR_TASK_PROTOCOL_MISMATCH: &str = "task.protocol_mismatch";

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    /// Constructors whose first argument is an error code.
    const CODE_FIRST_CONSTRUCTORS: &[&str] = &[
        "runtime_failure(",
        "runtime_error(",
        "RuntimeError::new(",
        "PluginHostError::new(",
        "sdk_error(",
        "sdk_resource_error(",
    ];

    /// Kernel packages whose error codes must come from this closed registry.
    const KERNEL_CRATES: &[&str] = &[
        "mutsuki-runtime-contracts",
        "mutsuki-runtime-wire",
        "mutsuki-runtime-core",
        "mutsuki-runtime-host",
        "mutsuki-runtime-sdk",
        "mutsuki-plugin-api",
        "mutsuki-plugin-host",
        "mutsuki-module-catalog",
    ];

    fn registered_codes() -> BTreeSet<String> {
        include_str!("error.rs")
            .lines()
            .filter_map(|line| line.strip_prefix("pub const ERR_"))
            .filter_map(|line| line.split('"').nth(1))
            .map(str::to_owned)
            .collect()
    }

    fn literal_codes(source: &str) -> Vec<String> {
        let mut codes = Vec::new();
        for constructor in CODE_FIRST_CONSTRUCTORS {
            for (index, _) in source.match_indices(constructor) {
                let rest = source[index + constructor.len()..].trim_start();
                if let Some(literal) = rest.strip_prefix('"')
                    && let Some(end) = literal.find('"')
                    && is_code_shaped(&literal[..end])
                {
                    codes.push(literal[..end].to_owned());
                }
            }
        }
        codes
    }

    /// `domain.name` in lowercase snake case; anything else is not an error code literal.
    fn is_code_shaped(literal: &str) -> bool {
        literal.contains('.')
            && literal
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '.')
    }

    fn rust_sources(directory: &Path, sources: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(directory).expect("read kernel source directory") {
            let path = entry.expect("read kernel source entry").path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name != "tests") {
                    rust_sources(&path, sources);
                }
            } else if path.extension().is_some_and(|extension| extension == "rs")
                && path.file_name().is_some_and(|name| name != "tests.rs")
            {
                sources.push(path);
            }
        }
    }

    /// Codes are a closed set mirrored by the Python kit: a literal that bypasses the registry
    /// is invisible to the mirror and to callers that branch on codes.
    #[test]
    fn kernel_error_codes_come_from_the_registry() {
        let registered = registered_codes();
        let crates_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("contracts lives under crates/");
        let mut unregistered = Vec::new();
        for package in KERNEL_CRATES {
            let mut sources = Vec::new();
            rust_sources(&crates_root.join(package).join("src"), &mut sources);
            for path in sources {
                let source = std::fs::read_to_string(&path).expect("read kernel source");
                for code in literal_codes(&source) {
                    if !registered.contains(&code) {
                        unregistered.push(format!("{}: {code}", path.display()));
                    }
                }
            }
        }
        assert!(
            unregistered.is_empty(),
            "register these codes as ERR_* constants and use the constant:\n{}",
            unregistered.join("\n")
        );
    }
}
