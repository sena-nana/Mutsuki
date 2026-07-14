use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use headless_chrome::{Browser, LaunchOptions};
use mutsuki_protocol_browser::{
    BrowserSnapshot, BrowserSnapshotRequest, BrowserWaitMode, SNAPSHOT, SNAPSHOT_SCHEMA,
};
use mutsuki_runtime_contracts::{
    CompletionBatch, ExecutionClass, PatchDescriptor, RunnerBatchCapability, RunnerContext,
    RunnerDescriptor, RunnerMode, RunnerPurity, RunnerResult, RunnerSideEffect, RuntimeError,
    ScalarValue, Task, WorkBatch, WritePlan,
};
use mutsuki_runtime_core::{Runner, RuntimeResult};
use mutsuki_runtime_sdk::{
    PluginBuilder, ProtocolDescriptorBuilder, ResourceRegistryGateway, RunnerDescriptorBuilder,
    map_work_batch_entries,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use url::Url;

pub const PLUGIN_ID: &str = "mutsuki.std.io.browser.chromium";
pub const RUNNER_ID: &str = "mutsuki.std.io.browser.chromium.runner";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChromiumConfig {
    pub executable: PathBuf,
    pub domain_allowlist: Vec<String>,
    pub timeout_ms: u64,
    pub max_dom_bytes: usize,
}

impl ChromiumConfig {
    pub fn validate(&self) -> Result<(), String> {
        if !self.executable.is_absolute() || !self.executable.is_file() {
            return Err(format!(
                "Chromium executable is missing or not an absolute file: {}",
                self.executable.display()
            ));
        }
        if self.domain_allowlist.is_empty()
            || self
                .domain_allowlist
                .iter()
                .any(|domain| normalize_domain(domain).is_none())
        {
            return Err("domain_allowlist must contain valid explicit DNS names".into());
        }
        if self.timeout_ms == 0 {
            return Err("timeout_ms must be greater than zero".into());
        }
        if self.max_dom_bytes == 0 {
            return Err("max_dom_bytes must be greater than zero".into());
        }
        Ok(())
    }
}

pub trait BrowserBackend: Send {
    fn snapshot(&mut self, request: &BrowserSnapshotRequest) -> Result<BrowserSnapshot, String>;
    fn cancel(&mut self, _invocation_id: &str) -> Result<(), String> {
        Ok(())
    }
    fn dispose(&mut self) -> Result<(), String> {
        Ok(())
    }
}

pub struct ChromiumBackend {
    browser: Option<Browser>,
}

impl ChromiumBackend {
    pub fn launch(config: &ChromiumConfig) -> Result<Self, String> {
        config.validate()?;
        let options = LaunchOptions::default_builder()
            .path(Some(config.executable.clone()))
            .headless(true)
            .sandbox(true)
            .idle_browser_timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .map_err(|error| format!("invalid Chromium launch options: {error}"))?;
        let browser = Browser::new(options)
            .map_err(|error| format!("failed to launch configured Chromium: {error}"))?;
        browser.set_default_timeout(Duration::from_millis(config.timeout_ms));
        Ok(Self {
            browser: Some(browser),
        })
    }
}

impl BrowserBackend for ChromiumBackend {
    fn snapshot(&mut self, request: &BrowserSnapshotRequest) -> Result<BrowserSnapshot, String> {
        let browser = self
            .browser
            .as_ref()
            .ok_or_else(|| "Chromium backend has been disposed".to_string())?;
        let tab = browser
            .new_tab()
            .map_err(|error| format!("failed to create Chromium tab: {error}"))?;
        tab.set_default_timeout(Duration::from_millis(request.timeout_ms));
        let result = (|| {
            tab.navigate_to(&request.url)
                .map_err(|error| format!("navigation failed: {error}"))?;
            tab.wait_until_navigated()
                .map_err(|error| format!("navigation timed out: {error}"))?;
            if request.wait_mode == BrowserWaitMode::Selector {
                let selector = request
                    .selector
                    .as_deref()
                    .ok_or_else(|| "selector wait mode requires selector".to_string())?;
                tab.wait_for_element_with_custom_timeout(
                    selector,
                    Duration::from_millis(request.timeout_ms),
                )
                .map_err(|error| format!("selector wait failed: {error}"))?;
            }
            Ok(BrowserSnapshot {
                final_url: tab.get_url(),
                title: tab
                    .get_title()
                    .map_err(|error| format!("title read failed: {error}"))?,
                html: tab
                    .get_content()
                    .map_err(|error| format!("DOM snapshot failed: {error}"))?,
            })
        })();
        let _ = tab.close(false);
        result
    }

    fn dispose(&mut self) -> Result<(), String> {
        self.browser.take();
        Ok(())
    }
}

pub struct BrowserSnapshotRunner {
    descriptor: RunnerDescriptor,
    config: ChromiumConfig,
    backend: Box<dyn BrowserBackend>,
    resources: Arc<dyn ResourceRegistryGateway>,
}

impl BrowserSnapshotRunner {
    pub fn launch(
        config: ChromiumConfig,
        resources: Arc<dyn ResourceRegistryGateway>,
    ) -> Result<Self, String> {
        let backend = ChromiumBackend::launch(&config)?;
        Ok(Self::with_backend(config, resources, Box::new(backend)))
    }

    pub fn with_backend(
        config: ChromiumConfig,
        resources: Arc<dyn ResourceRegistryGateway>,
        backend: Box<dyn BrowserBackend>,
    ) -> Self {
        Self {
            descriptor: runner_descriptor(),
            config,
            backend,
            resources,
        }
    }

    fn run_task(&mut self, task: &Task) -> Result<RunnerResult, RuntimeError> {
        let request: BrowserSnapshotRequest = serde_json::from_value(task.payload.clone())
            .map_err(|error| browser_error(task, "request.invalid", error.to_string()))?;
        validate_request(&self.config, &request)
            .map_err(|detail| browser_error(task, "request.denied", detail))?;
        let snapshot = self
            .backend
            .snapshot(&request)
            .map_err(|detail| browser_error(task, "snapshot.failed", detail))?;
        ensure_url_allowed(&self.config.domain_allowlist, &snapshot.final_url)
            .map_err(|detail| browser_error(task, "redirect.denied", detail))?;
        let bytes = serde_json::to_vec(&snapshot)
            .map_err(|error| browser_error(task, "snapshot.encode", error.to_string()))?;
        if bytes.len() > self.config.max_dom_bytes {
            return Err(browser_error(
                task,
                "snapshot.oversized",
                format!(
                    "snapshot is {} bytes; maximum is {}",
                    bytes.len(),
                    self.config.max_dom_bytes
                ),
            ));
        }
        let plan = write_plan(task, request.output_resource);
        let receipt = self
            .resources
            .commit_write_plan(&plan, bytes)
            .map_err(|error| browser_error(task, "snapshot.commit", error.to_string()))?;
        let descriptor = receipt
            .resource_ref
            .ok_or_else(|| browser_error(task, "snapshot.commit", "missing descriptor"))?;
        let mut result = RunnerResult::completed(task.task_id.clone());
        result.resources.push(descriptor);
        Ok(result)
    }
}

impl Runner for BrowserSnapshotRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        if ctx.cancel_requested {
            let error = RuntimeError::new(
                "task.cancelled",
                "mutsuki.std.browser.chromium",
                format!("browser.batch.cancelled.{}", ctx.invocation_id),
            );
            return Ok(CompletionBatch::from_error(&batch, error));
        }
        map_work_batch_entries(&batch, |task| self.run_task(task))
    }

    fn cancel(&mut self, invocation_id: &str) -> RuntimeResult<()> {
        self.backend.cancel(invocation_id).map_err(|detail| {
            mutsuki_runtime_core::RuntimeFailure::new(RuntimeError::new(
                "task.cancelled",
                "mutsuki.std.browser.chromium",
                format!("browser.cancel.{detail}"),
            ))
        })
    }

    fn dispose(&mut self) -> RuntimeResult<()> {
        self.backend.dispose().map_err(|detail| {
            mutsuki_runtime_core::RuntimeFailure::new(RuntimeError::new(
                mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
                "mutsuki.std.browser.chromium",
                format!("browser.dispose.{detail}"),
            ))
        })
    }
}

pub fn manifest() -> mutsuki_runtime_contracts::PluginManifest {
    PluginBuilder::new(PLUGIN_ID)
        .runner(Box::new(ManifestOnlyRunner {
            descriptor: runner_descriptor(),
        }))
        .protocol_handler(protocol_descriptor(), RUNNER_ID, "blocking")
        .build()
        .manifest
}

fn runner_descriptor() -> RunnerDescriptor {
    RunnerDescriptorBuilder::new(RUNNER_ID, PLUGIN_ID)
        .accepted_protocol(SNAPSHOT)
        .purity(RunnerPurity::Effectful)
        .execution_class(ExecutionClass::Blocking)
        .batch_capability(RunnerBatchCapability {
            mode: RunnerMode::ScalarAdapter,
            side_effect: RunnerSideEffect::External,
            ..Default::default()
        })
        .metadata(
            "standard_plugin",
            ScalarValue::String("browser_chromium".into()),
        )
        .build()
}

fn protocol_descriptor() -> mutsuki_runtime_contracts::ProtocolDescriptor {
    ProtocolDescriptorBuilder::new(SNAPSHOT)
        .input_schema(mutsuki_protocol_browser::input_schema(SNAPSHOT).unwrap())
        .output_schema(mutsuki_protocol_browser::output_schema(SNAPSHOT).unwrap())
        .error_schema(mutsuki_protocol_browser::error_schema(SNAPSHOT).unwrap())
        .build()
}

struct ManifestOnlyRunner {
    descriptor: RunnerDescriptor,
}

impl Runner for ManifestOnlyRunner {
    fn descriptor(&self) -> &RunnerDescriptor {
        &self.descriptor
    }

    fn run_batch(
        &mut self,
        _ctx: RunnerContext,
        batch: WorkBatch,
    ) -> RuntimeResult<CompletionBatch> {
        Ok(CompletionBatch::from_error(
            &batch,
            RuntimeError::new(
                mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
                "mutsuki.std.browser.chromium",
                "manifest_only_runner",
            ),
        ))
    }
}

fn validate_request(
    config: &ChromiumConfig,
    request: &BrowserSnapshotRequest,
) -> Result<(), String> {
    if request.timeout_ms == 0 || request.timeout_ms > config.timeout_ms {
        return Err(format!(
            "request timeout {} exceeds configured maximum {}",
            request.timeout_ms, config.timeout_ms
        ));
    }
    if request.output_resource.schema != SNAPSHOT_SCHEMA {
        return Err(format!("output resource schema must be {SNAPSHOT_SCHEMA}"));
    }
    if request.wait_mode == BrowserWaitMode::Selector
        && request.selector.as_deref().is_none_or(str::is_empty)
    {
        return Err("selector wait mode requires a non-empty selector".into());
    }
    ensure_url_allowed(&config.domain_allowlist, &request.url)
}

fn ensure_url_allowed(allowlist: &[String], value: &str) -> Result<(), String> {
    let url = Url::parse(value).map_err(|error| format!("invalid URL: {error}"))?;
    if url.scheme() != "https" {
        return Err("only https URLs are allowed".into());
    }
    let host = url
        .host_str()
        .map(|host| host.to_ascii_lowercase())
        .ok_or_else(|| "URL does not contain a DNS host".to_string())?;
    if allowlist.iter().any(|allowed| {
        normalize_domain(allowed)
            .is_some_and(|allowed| host == allowed || host.ends_with(&format!(".{allowed}")))
    }) {
        Ok(())
    } else {
        Err(format!("domain {host} is not in the configured allowlist"))
    }
}

fn normalize_domain(value: &str) -> Option<String> {
    let domain = value.trim().trim_start_matches('.').to_ascii_lowercase();
    (!domain.is_empty()
        && domain
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-')))
    .then_some(domain)
}

fn write_plan(task: &Task, resource: mutsuki_runtime_contracts::ResourceRef) -> WritePlan {
    let base_version = resource.version;
    WritePlan {
        plan_id: format!("browser.snapshot.write.{}", task.task_id),
        resource: resource.clone(),
        base_version,
        conflict_policy: "replace".into(),
        patch: PatchDescriptor {
            patch_id: format!("browser.snapshot.patch.{}", task.task_id),
            target_ref: resource,
            base_version,
            conflict_policy: "replace".into(),
            operations: json!({"replace": true}),
        },
        returning: None,
    }
}

fn browser_error(task: &Task, route: &str, detail: impl Into<String>) -> RuntimeError {
    let mut error = RuntimeError::new(
        "browser.snapshot_failed",
        "mutsuki.std.browser.chromium",
        format!("browser.{route}.{}", task.task_id),
    );
    error
        .evidence
        .insert("detail".into(), ScalarValue::String(detail.into()));
    error
}

#[cfg(test)]
mod tests {
    use mutsuki_runtime_contracts::{
        ResourceAccess, ResourceId, ResourceLifetime, ResourceRef, ResourceSealState,
        ResourceSemantic,
    };

    use super::*;

    fn config() -> ChromiumConfig {
        ChromiumConfig {
            executable: PathBuf::from("/does/not/matter/in/unit-tests"),
            domain_allowlist: vec!["mihuashi.com".into()],
            timeout_ms: 5_000,
            max_dom_bytes: 1024,
        }
    }

    fn output_resource() -> ResourceRef {
        ResourceRef {
            ref_id: "smoke-output".into(),
            resource_id: ResourceId {
                kind_id: "browser.snapshot".into(),
                slot_id: "smoke-output".into(),
                generation: 1,
                version: 1,
            },
            semantic: ResourceSemantic::CowVersionedState,
            provider_id: "mutsuki.std.resource.memory".into(),
            resource_kind: "browser.snapshot".into(),
            schema: SNAPSHOT_SCHEMA.into(),
            version: 1,
            generation: 1,
            access: ResourceAccess::ProviderRpc {
                provider_id: "mutsuki.std.resource.memory".into(),
                method: "memory".into(),
            },
            size_hint: Some(0),
            content_hash: None,
            lifetime: ResourceLifetime::Persistent,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        }
    }

    struct FakeBrowserBackend {
        disposed: bool,
    }

    impl BrowserBackend for FakeBrowserBackend {
        fn snapshot(
            &mut self,
            request: &BrowserSnapshotRequest,
        ) -> Result<BrowserSnapshot, String> {
            Ok(BrowserSnapshot {
                final_url: request.url.clone(),
                title: "fake".into(),
                html: "<main>ready</main>".into(),
            })
        }

        fn dispose(&mut self) -> Result<(), String> {
            self.disposed = true;
            Ok(())
        }
    }

    #[test]
    fn fake_backend_supports_deterministic_snapshot_and_dispose() {
        let mut backend = FakeBrowserBackend { disposed: false };
        let request = BrowserSnapshotRequest {
            url: "https://www.mihuashi.com/profile".into(),
            output_resource: output_resource(),
            wait_mode: BrowserWaitMode::Selector,
            selector: Some("main".into()),
            timeout_ms: 1_000,
        };
        let snapshot = backend.snapshot(&request).unwrap();
        assert_eq!(snapshot.title, "fake");
        assert!(snapshot.html.contains("ready"));
        backend.dispose().unwrap();
        assert!(backend.disposed);
    }

    #[test]
    fn domain_allowlist_rejects_http_and_unlisted_redirects() {
        assert!(ensure_url_allowed(&config().domain_allowlist, "http://mihuashi.com").is_err());
        assert!(
            ensure_url_allowed(&config().domain_allowlist, "https://evil.example/profile").is_err()
        );
        assert!(
            ensure_url_allowed(
                &config().domain_allowlist,
                "https://www.mihuashi.com/profile"
            )
            .is_ok()
        );
    }

    #[test]
    fn configured_limits_reject_zero_timeout_and_oversized_dom() {
        let mut invalid = config();
        invalid.timeout_ms = 0;
        assert!(invalid.validate().is_err());
        let snapshot = BrowserSnapshot {
            final_url: "https://www.mihuashi.com/profile".into(),
            title: "profile".into(),
            html: "x".repeat(2048),
        };
        assert!(serde_json::to_vec(&snapshot).unwrap().len() > config().max_dom_bytes);
    }

    #[test]
    #[ignore = "requires an explicit local CHROMIUM_EXECUTABLE"]
    fn real_chromium_smoke() {
        let executable = std::env::var("CHROMIUM_EXECUTABLE").unwrap();
        let config = ChromiumConfig {
            executable: executable.into(),
            domain_allowlist: vec!["example.com".into()],
            timeout_ms: 10_000,
            max_dom_bytes: 1024 * 1024,
        };
        let mut backend = ChromiumBackend::launch(&config).unwrap();
        let request = BrowserSnapshotRequest {
            url: "https://example.com".into(),
            output_resource: output_resource(),
            wait_mode: BrowserWaitMode::Load,
            selector: None,
            timeout_ms: 10_000,
        };
        let snapshot = backend.snapshot(&request).unwrap();
        assert!(snapshot.html.contains("Example Domain"));
    }
}
