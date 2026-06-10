use mutsuki_runtime_contracts::{
    Envelope, LeaseToken, OperationHandlerKey, OperationSnapshot, OperationStatus, PluginSnapshot,
    RefDescriptor, ResourceRecord, SourceSnapshot, StrategyResult,
};
use serde_json::Value;

use crate::RuntimeResult;

#[derive(Clone, Debug, PartialEq)]
pub enum BackendPayload {
    Json(Value),
    Envelope(Envelope),
}

pub trait StrategyBackend {
    fn on_awake(&mut self, agent_id: &str) -> RuntimeResult<()>;
    fn on_input(&mut self, agent_id: &str, envelope: &Envelope) -> RuntimeResult<StrategyResult>;
    fn next_step(&mut self, agent_id: &str) -> RuntimeResult<StrategyResult>;
    fn on_stop(&mut self, agent_id: &str) -> RuntimeResult<()>;
}

pub trait OperationBackend {
    fn list_plugins(&self) -> RuntimeResult<Vec<PluginSnapshot>>;
    fn list_operations(
        &self,
        enabled_plugin_ids: &[String],
    ) -> RuntimeResult<Vec<OperationSnapshot>>;
    fn list_sources(&self, enabled_plugin_ids: &[String]) -> RuntimeResult<Vec<SourceSnapshot>>;
    fn invoke(
        &mut self,
        agent_id: &str,
        key: &OperationHandlerKey,
        payload: Value,
    ) -> RuntimeResult<BackendPayload>;
    fn operation_status(&self, agent_id: &str, key: &OperationHandlerKey) -> OperationStatus;
}

pub trait ResourceBackend {
    fn register_resource(
        &mut self,
        descriptor: RefDescriptor,
        owner: &str,
    ) -> RuntimeResult<String>;
    fn acquire_resource(&mut self, ref_id: &str, requester: &str) -> RuntimeResult<LeaseToken>;
    fn release_resource(&mut self, token: &LeaseToken) -> RuntimeResult<()>;
    fn list_records(&self, owner: Option<&str>) -> Vec<ResourceRecord>;
}

pub trait RuntimeBackend: StrategyBackend + OperationBackend {}

impl<T> RuntimeBackend for T where T: StrategyBackend + OperationBackend {}
