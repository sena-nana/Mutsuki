use std::collections::BTreeMap;
use std::sync::Arc;

use mutsuki_runtime_contracts::{PluginManifest, RuntimeError};
use mutsuki_runtime_core::{Runner, RuntimeFailure, RuntimeResult};
use mutsuki_runtime_wire::{
    AnyWireRequest, DEBUG_JSONL_CODEC_ID, DecodedWireRequest, InitializedPlugin, Opcode,
    ProtocolHello, ProtocolHelloAck, decode_jsonl_any_request,
};
use serde_json::Value;

use crate::{LoadedPlugin, ResourceProviderGateway};

use super::error::{abi_failure, encode_result};
use super::types::AbiGuest;

pub struct JsonlPluginGuest {
    manifest: PluginManifest,
    runners: BTreeMap<String, Box<dyn Runner>>,
    providers: BTreeMap<String, Arc<dyn ResourceProviderGateway>>,
    initialized: bool,
}

impl JsonlPluginGuest {
    pub fn new(plugin: LoadedPlugin) -> RuntimeResult<Self> {
        if !plugin.host_services.is_empty() {
            return Err(abi_failure(
                "abi.host_service_unsupported",
                "ABI plugins cannot export host services",
            ));
        }
        let mut runners = BTreeMap::new();
        for runner in plugin.runners {
            let runner_id = runner.descriptor().runner_id.clone();
            if runners.insert(runner_id.clone(), runner).is_some() {
                return Err(abi_failure("abi.runner_duplicate", runner_id));
            }
        }
        let mut providers = BTreeMap::new();
        for provider in plugin.resource_providers {
            if providers
                .insert(provider.provider_id.clone(), provider.provider)
                .is_some()
            {
                return Err(abi_failure("abi.provider_duplicate", provider.provider_id));
            }
        }
        Ok(Self {
            manifest: plugin.manifest,
            runners,
            providers,
            initialized: false,
        })
    }

    fn initialize(&mut self, hello: ProtocolHello) -> RuntimeResult<ProtocolHelloAck> {
        if self.initialized {
            return Err(abi_failure(
                "abi.already_initialized",
                "plugin.initialize may only be called once",
            ));
        }
        let plugin = InitializedPlugin {
            manifest: self.manifest.clone(),
            resource_provider_ids: self.providers.keys().cloned().collect(),
        };
        let ack = hello
            .accept(DEBUG_JSONL_CODEC_ID, Some(plugin))
            .map_err(|error| abi_failure("abi.handshake", error.to_string()))?;
        self.initialized = true;
        Ok(ack)
    }

    fn handle(&mut self, decoded: DecodedWireRequest) -> Vec<u8> {
        let request_id = decoded.request_id;
        if let AnyWireRequest::Initialize(request) = decoded.request {
            return encode_result(
                request_id,
                Opcode::PluginInitialize,
                self.initialize(request.hello),
            );
        }
        if !self.initialized {
            return encode_result::<()>(
                request_id,
                decoded.request.opcode(),
                Err(abi_failure(
                    "abi.not_initialized",
                    "plugin.initialize must precede business requests",
                )),
            );
        }
        self.dispatch(request_id, decoded.request)
    }

    fn dispatch(&mut self, request_id: u64, request: AnyWireRequest) -> Vec<u8> {
        match request {
            AnyWireRequest::RunBatch(request) => {
                let result = self
                    .runner(&request.runner_id)
                    .and_then(|runner| runner.run_batch(request.ctx, request.batch));
                encode_result(request_id, Opcode::RunnerRunBatch, result)
            }
            AnyWireRequest::CancelRunner(request) => {
                let result = self
                    .runner(&request.runner_id)
                    .and_then(|runner| runner.cancel(&request.invocation_id));
                encode_result(request_id, Opcode::RunnerCancel, result)
            }
            AnyWireRequest::DisposeRunner(request) => {
                let result = self
                    .runner(&request.runner_id)
                    .and_then(|runner| runner.dispose());
                encode_result(request_id, Opcode::RunnerDispose, result)
            }
            AnyWireRequest::CreateBlob(request) => {
                let result = self
                    .provider(request.provider_id.as_deref())
                    .and_then(|provider| {
                        provider.create_blob_resource(&request.schema, request.bytes)
                    });
                encode_result(request_id, Opcode::ResourceCreateBlob, result)
            }
            AnyWireRequest::CreateCowState(request) => {
                let result = self
                    .provider(request.provider_id.as_deref())
                    .and_then(|provider| {
                        provider.create_cow_state_resource(
                            &request.kind_id,
                            &request.schema,
                            request.bytes,
                        )
                    });
                encode_result(request_id, Opcode::ResourceCreateCowState, result)
            }
            AnyWireRequest::CreateCapability(request) => {
                let result = self
                    .provider(request.provider_id.as_deref())
                    .and_then(|provider| {
                        provider.create_capability_resource(&request.kind_id, &request.schema)
                    });
                encode_result(request_id, Opcode::ResourceCreateCapability, result)
            }
            AnyWireRequest::CollectReadPlan(request) => {
                let result = self
                    .provider(request.provider_id.as_deref())
                    .and_then(|provider| provider.collect_read_plan(&request.plan));
                encode_result(request_id, Opcode::ResourceReadCollect, result)
            }
            AnyWireRequest::SnapshotReadPlan(request) => {
                let result = self
                    .provider(request.provider_id.as_deref())
                    .and_then(|provider| {
                        provider.snapshot_read_plan(
                            &request.plan,
                            &request.kind_id,
                            &request.schema,
                        )
                    });
                encode_result(request_id, Opcode::ResourceReadSnapshot, result)
            }
            AnyWireRequest::OpenStreamPlan(request) => {
                let result = self
                    .provider(request.provider_id.as_deref())
                    .and_then(|provider| provider.open_stream_plan(&request.plan));
                encode_result(request_id, Opcode::ResourceStreamOpen, result)
            }
            AnyWireRequest::ExportPlan(request) => {
                let result = self
                    .provider(request.provider_id.as_deref())
                    .and_then(|provider| provider.execute_export_plan(&request.plan));
                encode_result(request_id, Opcode::ResourceExport, result)
            }
            AnyWireRequest::CommitWritePlan(request) => {
                let result = self
                    .provider(request.provider_id.as_deref())
                    .and_then(|provider| provider.commit_write_plan(&request.plan, request.bytes));
                encode_result(request_id, Opcode::ResourceWriteCommit, result)
            }
            AnyWireRequest::CommandPlan(request) => {
                let result = self
                    .provider(request.provider_id.as_deref())
                    .and_then(|provider| provider.execute_command_plan(&request.plan));
                encode_result(request_id, Opcode::ResourceCommand, result)
            }
            AnyWireRequest::CommandBatch(request) => {
                let result = self
                    .provider(request.provider_id.as_deref())
                    .and_then(|provider| provider.execute_command_batch(&request.batch));
                encode_result(request_id, Opcode::ResourceCommandBatch, result)
            }
            AnyWireRequest::SagaPlan(request) => {
                let result = self
                    .provider(request.provider_id.as_deref())
                    .and_then(|provider| provider.execute_saga_plan(&request.saga));
                encode_result(request_id, Opcode::ResourceSaga, result)
            }
            unsupported => encode_result::<()>(
                request_id,
                unsupported.opcode(),
                Err(abi_failure(
                    "abi.guest_opcode_unsupported",
                    format!(
                        "unsupported guest opcode {:#06x}",
                        unsupported.opcode() as u16
                    ),
                )),
            ),
        }
    }

    fn runner(&mut self, runner_id: &str) -> RuntimeResult<&mut Box<dyn Runner>> {
        self.runners
            .get_mut(runner_id)
            .ok_or_else(|| abi_failure("abi.runner_not_found", runner_id))
    }

    fn provider(
        &self,
        provider_id: Option<&str>,
    ) -> RuntimeResult<&Arc<dyn ResourceProviderGateway>> {
        let provider_id = provider_id
            .ok_or_else(|| abi_failure("abi.provider_missing", "provider_id is required"))?;
        self.providers
            .get(provider_id)
            .ok_or_else(|| abi_failure("abi.provider_not_found", provider_id))
    }
}

impl AbiGuest for JsonlPluginGuest {
    fn request(&mut self, request: &[u8]) -> Vec<u8> {
        match decode_jsonl_any_request(request, mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS) {
            Ok(decoded) => self.handle(decoded),
            Err(error) => serde_json::to_vec(&RuntimeError::new(
                mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
                "abi.guest",
                format!("abi.decode:{error}"),
            ))
            .unwrap_or_default(),
        }
    }
}

type ConfiguredPluginFactory =
    Box<dyn FnOnce(Value) -> RuntimeResult<LoadedPlugin> + Send + 'static>;

pub struct ConfiguredJsonlPluginGuest {
    factory: Option<ConfiguredPluginFactory>,
    plugin: Option<JsonlPluginGuest>,
    initialization_attempted: bool,
}

impl ConfiguredJsonlPluginGuest {
    pub fn new(factory: ConfiguredPluginFactory) -> Self {
        Self {
            factory: Some(factory),
            plugin: None,
            initialization_attempted: false,
        }
    }
}

impl AbiGuest for ConfiguredJsonlPluginGuest {
    fn request(&mut self, bytes: &[u8]) -> Vec<u8> {
        let decoded =
            match decode_jsonl_any_request(bytes, mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS) {
                Ok(decoded) => decoded,
                Err(error) => {
                    return serde_json::to_vec(&RuntimeError::new(
                        mutsuki_runtime_contracts::ERR_RUNTIME_HOST_FAILED,
                        "abi.guest",
                        format!("abi.decode:{error}"),
                    ))
                    .unwrap_or_default();
                }
            };
        if let Some(plugin) = self.plugin.as_mut() {
            return plugin.handle(decoded);
        }
        let request_id = decoded.request_id;
        let AnyWireRequest::Initialize(request) = decoded.request else {
            return encode_result::<()>(
                request_id,
                decoded.request.opcode(),
                Err(abi_failure(
                    "abi.not_initialized",
                    "plugin.initialize must precede business requests",
                )),
            );
        };
        if self.initialization_attempted {
            return encode_result::<()>(
                request_id,
                Opcode::PluginInitialize,
                Err(abi_failure(
                    "abi.already_initialized",
                    "plugin.initialize may only be called once",
                )),
            );
        }
        self.initialization_attempted = true;
        let config = request.config.unwrap_or(Value::Null);
        let result = self
            .factory
            .take()
            .ok_or_else(|| abi_failure("abi.factory_missing", "plugin factory unavailable"))
            .and_then(|factory| factory(config))
            .and_then(JsonlPluginGuest::new)
            .and_then(|mut plugin| {
                let ack = plugin.initialize(request.hello)?;
                self.plugin = Some(plugin);
                Ok(ack)
            });
        encode_result(request_id, Opcode::PluginInitialize, result)
    }
}

pub struct FailedAbiGuest {
    error: RuntimeError,
}

impl FailedAbiGuest {
    pub fn new(error: RuntimeFailure) -> Self {
        Self {
            error: error.error().clone(),
        }
    }
}

impl AbiGuest for FailedAbiGuest {
    fn request(&mut self, request: &[u8]) -> Vec<u8> {
        match decode_jsonl_any_request(request, mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS) {
            Ok(decoded) => encode_result::<()>(
                decoded.request_id,
                decoded.request.opcode(),
                Err(RuntimeFailure::new(self.error.clone())),
            ),
            Err(_) => serde_json::to_vec(&self.error).unwrap_or_default(),
        }
    }
}
