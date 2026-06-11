use mutsukicore_runtime_contracts::{
    OperationDescriptor, OperationHandlerKey, OperationSnapshot, OperationStatus,
};
use mutsukicore_runtime_core::{BackendPayload, RuntimeResult};
use serde_json::Value;

pub(crate) type NativeHandler = Box<dyn FnMut(Value) -> RuntimeResult<BackendPayload>>;

pub struct NativeOperation {
    pub(crate) snapshot: OperationSnapshot,
    pub(crate) handler: NativeHandler,
}

impl NativeOperation {
    pub fn new(
        descriptor: OperationDescriptor,
        handler: impl FnMut(Value) -> RuntimeResult<BackendPayload> + 'static,
    ) -> Self {
        let key = OperationHandlerKey {
            plugin_id: descriptor.plugin_id.clone(),
            plugin_generation: 0,
            op_id: descriptor.op_id.clone(),
            handler_id: format!("{}:{}:0", descriptor.plugin_id, descriptor.op_id),
        };
        Self {
            snapshot: OperationSnapshot {
                descriptor,
                status: OperationStatus::Active,
                key,
            },
            handler: Box::new(handler),
        }
    }
}
