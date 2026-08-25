use std::sync::{Mutex, OnceLock};

use mutsuki_runtime_contracts::RuntimeError;
use mutsuki_runtime_core::{RuntimeFailure, RuntimeResult};
use mutsuki_runtime_wire::{
    AnyWireRequest, Opcode, decode_binary_any_request, decode_binary_frame,
};
use serde_json::Value;

use super::error::{abi_failure, encode_binary_result};
use super::guest::{BinaryGuestCodec, ConfiguredPluginFactory, PluginGuest};
use super::types::AbiGuest;
use crate::LoadedPlugin;

pub struct BinaryPluginGuest {
    plugin: PluginGuest,
}

impl BinaryPluginGuest {
    pub fn new(plugin: LoadedPlugin) -> RuntimeResult<Self> {
        Ok(Self {
            plugin: PluginGuest::new(plugin)?,
        })
    }
}

impl AbiGuest for BinaryPluginGuest {
    fn request(&self, request: &[u8]) -> Vec<u8> {
        match decode_binary_any_request(request, mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS) {
            Ok(decoded) => self.plugin.handle::<BinaryGuestCodec>(decoded),
            Err(_) => Vec::new(),
        }
    }
}

pub struct ConfiguredBinaryPluginGuest {
    /// Guards the one-shot construction only. Once the plugin exists, requests reach it through
    /// `OnceLock` without touching this lock, so the handshake does not serialise later traffic.
    factory: Mutex<Option<ConfiguredPluginFactory>>,
    plugin: OnceLock<PluginGuest>,
}

impl ConfiguredBinaryPluginGuest {
    pub fn new(factory: ConfiguredPluginFactory) -> Self {
        Self {
            factory: Mutex::new(Some(factory)),
            plugin: OnceLock::new(),
        }
    }
}

impl AbiGuest for ConfiguredBinaryPluginGuest {
    fn request(&self, bytes: &[u8]) -> Vec<u8> {
        let decoded =
            match decode_binary_any_request(bytes, mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS) {
                Ok(decoded) => decoded,
                Err(_) => return Vec::new(),
            };
        if let Some(plugin) = self.plugin.get() {
            return plugin.handle::<BinaryGuestCodec>(decoded);
        }
        let request_id = decoded.request_id;
        let AnyWireRequest::Initialize(request) = decoded.request else {
            return encode_binary_result::<()>(
                request_id,
                decoded.request.opcode(),
                Err(abi_failure(
                    "abi.not_initialized",
                    "plugin.initialize must precede business requests",
                )),
            );
        };
        let mut factory = self
            .factory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.plugin.get().is_some() {
            return encode_binary_result::<()>(
                request_id,
                Opcode::PluginInitialize,
                Err(abi_failure(
                    "abi.already_initialized",
                    "plugin.initialize may only be called once",
                )),
            );
        }
        let config = request.config.unwrap_or(Value::Null);
        // Taking the factory before running it makes a failed handshake terminal: the plugin was
        // already told why it was rejected, and retrying would re-run construction side effects.
        let result = factory
            .take()
            .ok_or_else(|| abi_failure("abi.factory_missing", "plugin factory unavailable"))
            .and_then(|factory| factory(config))
            .and_then(PluginGuest::new)
            .and_then(|plugin| {
                let ack = plugin.initialize::<BinaryGuestCodec>(request.hello)?;
                let _ = self.plugin.set(plugin);
                Ok(ack)
            });
        encode_binary_result(request_id, Opcode::PluginInitialize, result)
    }
}

pub struct FailedBinaryAbiGuest {
    error: RuntimeError,
}

impl FailedBinaryAbiGuest {
    pub fn new(error: RuntimeFailure) -> Self {
        Self {
            error: error.error().clone(),
        }
    }
}

impl AbiGuest for FailedBinaryAbiGuest {
    fn request(&self, request: &[u8]) -> Vec<u8> {
        match decode_binary_frame(request, mutsuki_runtime_wire::DEFAULT_WIRE_LIMITS) {
            Ok(frame) => encode_binary_result::<()>(
                frame.header.request_id,
                frame.header.opcode,
                Err(RuntimeFailure::new(self.error.clone())),
            ),
            Err(_) => Vec::new(),
        }
    }
}
