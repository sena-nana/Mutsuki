use std::collections::BTreeMap;
use std::sync::Arc;

use mutsuki_runtime_contracts::{PluginManifest, RunnerId};
use mutsuki_runtime_core::{Runner, RuntimeResult};
use mutsuki_runtime_wire::{
    AnyWireRequest, BINARY_CODEC_ID, DecodedWireRequest, InitializedPlugin, Opcode, ProtocolHello,
    ProtocolHelloAck,
};
use serde_json::Value;

use crate::{LoadedPlugin, ResourceProviderGateway};

use super::error::{abi_failure, encode_binary_result};

pub(super) struct PluginGuest {
    manifest: PluginManifest,
    runners: BTreeMap<RunnerId, Box<dyn Runner>>,
    providers: BTreeMap<String, Arc<dyn ResourceProviderGateway>>,
    initialized: bool,
}

pub(super) trait GuestResponseCodec {
    const CODEC_ID: &'static str;

    fn encode<T: serde::Serialize>(
        request_id: u64,
        opcode: Opcode,
        result: RuntimeResult<T>,
    ) -> Vec<u8>;
}

pub(super) struct BinaryGuestCodec;

impl GuestResponseCodec for BinaryGuestCodec {
    const CODEC_ID: &'static str = BINARY_CODEC_ID;

    fn encode<T: serde::Serialize>(
        request_id: u64,
        opcode: Opcode,
        result: RuntimeResult<T>,
    ) -> Vec<u8> {
        encode_binary_result(request_id, opcode, result)
    }
}

impl PluginGuest {
    pub(super) fn new(plugin: LoadedPlugin) -> RuntimeResult<Self> {
        if !plugin.host_services.is_empty() {
            return Err(abi_failure(
                "abi.host_service_unsupported",
                "ABI plugins cannot export host services",
            ));
        }
        let mut runners = BTreeMap::new();
        for runner in plugin.runners {
            let runner_id = RunnerId::from(runner.descriptor().runner_id.clone());
            if runners.insert(runner_id.clone(), runner).is_some() {
                return Err(abi_failure("abi.runner_duplicate", runner_id.to_string()));
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

    pub(super) fn initialize<C: GuestResponseCodec>(
        &mut self,
        hello: ProtocolHello,
    ) -> RuntimeResult<ProtocolHelloAck> {
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
            .accept(C::CODEC_ID, Some(plugin))
            .map_err(|error| abi_failure("abi.handshake", error.to_string()))?;
        self.initialized = true;
        Ok(ack)
    }

    pub(super) fn handle<C: GuestResponseCodec>(&mut self, decoded: DecodedWireRequest) -> Vec<u8> {
        let request_id = decoded.request_id;
        if let AnyWireRequest::Initialize(request) = decoded.request {
            return C::encode(
                request_id,
                Opcode::PluginInitialize,
                self.initialize::<C>(request.hello),
            );
        }
        if !self.initialized {
            return C::encode::<()>(
                request_id,
                decoded.request.opcode(),
                Err(abi_failure(
                    "abi.not_initialized",
                    "plugin.initialize must precede business requests",
                )),
            );
        }
        self.dispatch::<C>(request_id, decoded.request)
    }

    fn dispatch<C: GuestResponseCodec>(
        &mut self,
        request_id: u64,
        request: AnyWireRequest,
    ) -> Vec<u8> {
        match request {
            AnyWireRequest::RunBatch(request) => {
                let result = self
                    .runner(&request.runner_id)
                    .and_then(|runner| runner.run_batch(request.ctx, request.batch));
                C::encode(request_id, Opcode::RunnerRunBatch, result)
            }
            AnyWireRequest::CancelRunner(request) => {
                let result = self
                    .runner(&request.runner_id)
                    .and_then(|runner| runner.cancel(&request.invocation_id));
                C::encode(request_id, Opcode::RunnerCancel, result)
            }
            AnyWireRequest::DisposeRunner(request) => {
                let result = self
                    .runner(&request.runner_id)
                    .and_then(|runner| runner.dispose());
                C::encode(request_id, Opcode::RunnerDispose, result)
            }
            AnyWireRequest::CreateBlob(request) => {
                let result = self
                    .provider(request.provider_id.as_deref())
                    .and_then(|provider| {
                        provider.create_blob_resource(&request.schema, request.bytes)
                    });
                C::encode(request_id, Opcode::ResourceCreateBlob, result)
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
                C::encode(request_id, Opcode::ResourceCreateCowState, result)
            }
            AnyWireRequest::CreateCapability(request) => {
                let result = self
                    .provider(request.provider_id.as_deref())
                    .and_then(|provider| {
                        provider.create_capability_resource(&request.kind_id, &request.schema)
                    });
                C::encode(request_id, Opcode::ResourceCreateCapability, result)
            }
            AnyWireRequest::CollectReadPlan(request) => {
                let result = self
                    .provider(request.provider_id.as_deref())
                    .and_then(|provider| provider.collect_read_plan(&request.plan));
                C::encode(request_id, Opcode::ResourceReadCollect, result)
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
                C::encode(request_id, Opcode::ResourceReadSnapshot, result)
            }
            AnyWireRequest::OpenStreamPlan(request) => {
                let result = self
                    .provider(request.provider_id.as_deref())
                    .and_then(|provider| provider.open_stream_plan(&request.plan));
                C::encode(request_id, Opcode::ResourceStreamOpen, result)
            }
            AnyWireRequest::ExportPlan(request) => {
                let result = self
                    .provider(request.provider_id.as_deref())
                    .and_then(|provider| provider.execute_export_plan(&request.plan));
                C::encode(request_id, Opcode::ResourceExport, result)
            }
            AnyWireRequest::CommitWritePlan(request) => {
                let result = self
                    .provider(request.provider_id.as_deref())
                    .and_then(|provider| provider.commit_write_plan(&request.plan, request.bytes));
                C::encode(request_id, Opcode::ResourceWriteCommit, result)
            }
            AnyWireRequest::CommandPlan(request) => {
                let result = self
                    .provider(request.provider_id.as_deref())
                    .and_then(|provider| provider.execute_command_plan(&request.plan));
                C::encode(request_id, Opcode::ResourceCommand, result)
            }
            AnyWireRequest::CommandBatch(request) => {
                let result = self
                    .provider(request.provider_id.as_deref())
                    .and_then(|provider| provider.execute_command_batch(&request.batch));
                C::encode(request_id, Opcode::ResourceCommandBatch, result)
            }
            AnyWireRequest::SagaPlan(request) => {
                let result = self
                    .provider(request.provider_id.as_deref())
                    .and_then(|provider| provider.execute_saga_plan(&request.saga));
                C::encode(request_id, Opcode::ResourceSaga, result)
            }
            unsupported => C::encode::<()>(
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

pub(super) type ConfiguredPluginFactory =
    Box<dyn FnOnce(Value) -> RuntimeResult<LoadedPlugin> + Send + 'static>;
