use std::collections::BTreeMap;

use mutsuki_runtime_contracts::{
    ExecutionClass, HandlerBinding, ProtocolDescriptor, ResourceProviderCompatibility,
    ResourceProviderReloadPolicy, ResourceSemantic, ResourceTypeDescriptor, RunnerBatchCapability,
    RunnerControlCapability, RunnerDescriptor, RunnerOrderingCapability, RunnerPayloadCapability,
    RunnerPurity, RunnerResourceCapability, ScalarValue,
};
use serde_json::{Value, json};

use crate::{ResourceKind, SdkProtocol};

pub trait ProtocolSpec: SdkProtocol {
    fn version() -> &'static str {
        "1.0.0"
    }

    fn input_schema() -> Value {
        json!({})
    }

    fn output_schema() -> Value {
        json!({})
    }

    fn error_schema() -> Value {
        json!({})
    }

    fn codec() -> &'static str {
        "json"
    }

    fn compatibility() -> &'static str {
        "compatible"
    }

    fn descriptor() -> ProtocolDescriptor {
        ProtocolDescriptorBuilder::new(Self::PROTOCOL_ID)
            .version(Self::version())
            .input_schema(Self::input_schema())
            .output_schema(Self::output_schema())
            .error_schema(Self::error_schema())
            .codec(Self::codec())
            .compatibility(Self::compatibility())
            .build()
    }
}

pub trait ResourceKindSpec: ResourceKind {
    fn schema() -> &'static str;

    fn provider_id() -> &'static str;

    fn operations() -> &'static [&'static str] {
        &[]
    }

    fn reload_policy() -> ResourceProviderReloadPolicy {
        ResourceProviderReloadPolicy::CompatibleWithoutLeases
    }

    fn descriptor() -> ResourceTypeDescriptor {
        ResourceTypeDescriptorBuilder::new(Self::KIND_ID, Self::SEMANTIC)
            .schema(Self::schema())
            .provider_id(Self::provider_id())
            .operations(Self::operations().iter().copied())
            .reload_policy(Self::reload_policy())
            .build()
    }
}

#[derive(Clone, Debug)]
pub struct ProtocolDescriptorBuilder {
    protocol_id: String,
    version: String,
    input_schema: Value,
    output_schema: Value,
    error_schema: Value,
    codec: String,
    compatibility: String,
}

impl ProtocolDescriptorBuilder {
    pub fn new(protocol_id: impl Into<String>) -> Self {
        Self {
            protocol_id: protocol_id.into(),
            version: "1.0.0".into(),
            input_schema: json!({}),
            output_schema: json!({}),
            error_schema: json!({}),
            codec: "json".into(),
            compatibility: "compatible".into(),
        }
    }

    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    pub fn input_schema(mut self, schema: Value) -> Self {
        self.input_schema = schema;
        self
    }

    pub fn output_schema(mut self, schema: Value) -> Self {
        self.output_schema = schema;
        self
    }

    pub fn error_schema(mut self, schema: Value) -> Self {
        self.error_schema = schema;
        self
    }

    pub fn codec(mut self, codec: impl Into<String>) -> Self {
        self.codec = codec.into();
        self
    }

    pub fn compatibility(mut self, compatibility: impl Into<String>) -> Self {
        self.compatibility = compatibility.into();
        self
    }

    pub fn build(self) -> ProtocolDescriptor {
        ProtocolDescriptor {
            protocol_id: self.protocol_id,
            version: self.version,
            input_schema: self.input_schema,
            output_schema: self.output_schema,
            error_schema: self.error_schema,
            codec: self.codec,
            compatibility: self.compatibility,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RunnerDescriptorBuilder {
    runner_id: String,
    plugin_id: String,
    plugin_generation: u64,
    accepted_protocol_ids: Vec<String>,
    purity: RunnerPurity,
    execution_class: ExecutionClass,
    input_schema: Value,
    output_schema: Value,
    batch: RunnerBatchCapability,
    payload: RunnerPayloadCapability,
    resources: RunnerResourceCapability,
    ordering: RunnerOrderingCapability,
    control: RunnerControlCapability,
    metadata: BTreeMap<String, ScalarValue>,
    contract_surfaces: Vec<String>,
}

impl RunnerDescriptorBuilder {
    pub fn new(runner_id: impl Into<String>, plugin_id: impl Into<String>) -> Self {
        let runner_id = runner_id.into();
        Self {
            contract_surfaces: vec![format!("runner:{runner_id}")],
            runner_id,
            plugin_id: plugin_id.into(),
            plugin_generation: 1,
            accepted_protocol_ids: Vec::new(),
            purity: RunnerPurity::Pure,
            execution_class: ExecutionClass::Cpu,
            input_schema: json!({}),
            output_schema: json!({}),
            batch: RunnerBatchCapability::default(),
            payload: RunnerPayloadCapability::default(),
            resources: RunnerResourceCapability::default(),
            ordering: RunnerOrderingCapability::default(),
            control: RunnerControlCapability::default(),
            metadata: BTreeMap::new(),
        }
    }

    pub fn plugin_generation(mut self, plugin_generation: u64) -> Self {
        self.plugin_generation = plugin_generation;
        self
    }

    pub fn accepts<P>(mut self) -> Self
    where
        P: SdkProtocol,
    {
        self.accepted_protocol_ids.push(P::PROTOCOL_ID.into());
        self
    }

    pub fn accepted_protocol(mut self, protocol_id: impl Into<String>) -> Self {
        self.accepted_protocol_ids.push(protocol_id.into());
        self
    }

    pub fn purity(mut self, purity: RunnerPurity) -> Self {
        self.purity = purity;
        self
    }

    pub fn execution_class(mut self, execution_class: ExecutionClass) -> Self {
        self.execution_class = execution_class;
        self
    }

    pub fn input_schema(mut self, schema: Value) -> Self {
        self.input_schema = schema;
        self
    }

    pub fn output_schema(mut self, schema: Value) -> Self {
        self.output_schema = schema;
        self
    }

    pub fn batch_capability(mut self, capability: RunnerBatchCapability) -> Self {
        self.batch = capability;
        self
    }

    pub fn payload_capability(mut self, capability: RunnerPayloadCapability) -> Self {
        self.payload = capability;
        self
    }

    pub fn resource_capability(mut self, capability: RunnerResourceCapability) -> Self {
        self.resources = capability;
        self
    }

    pub fn ordering_capability(mut self, capability: RunnerOrderingCapability) -> Self {
        self.ordering = capability;
        self
    }

    pub fn control_capability(mut self, capability: RunnerControlCapability) -> Self {
        self.control = capability;
        self
    }

    pub fn metadata(mut self, key: impl Into<String>, value: ScalarValue) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    pub fn contract_surface(mut self, surface_id: impl Into<String>) -> Self {
        self.contract_surfaces.push(surface_id.into());
        self
    }

    pub fn build(self) -> RunnerDescriptor {
        RunnerDescriptor {
            runner_id: self.runner_id,
            plugin_id: self.plugin_id,
            plugin_generation: self.plugin_generation,
            accepted_protocol_ids: self.accepted_protocol_ids,
            purity: self.purity,
            execution_class: self.execution_class,
            input_schema: self.input_schema,
            output_schema: self.output_schema,
            batch: self.batch,
            payload: self.payload,
            resources: self.resources,
            ordering: self.ordering,
            control: self.control,
            metadata: self.metadata,
            contract_surfaces: self.contract_surfaces,
        }
    }
}

#[derive(Clone, Debug)]
pub struct HandlerBindingBuilder {
    binding_id: String,
    plugin_id: String,
    protocol_id: String,
    target_protocol_id: String,
    target_runner_hint: Option<String>,
    pool_id: String,
    priority: i64,
    policy: String,
    metadata: BTreeMap<String, ScalarValue>,
}

impl HandlerBindingBuilder {
    pub fn new(
        binding_id: impl Into<String>,
        plugin_id: impl Into<String>,
        protocol_id: impl Into<String>,
        target_protocol_id: impl Into<String>,
    ) -> Self {
        Self {
            binding_id: binding_id.into(),
            plugin_id: plugin_id.into(),
            protocol_id: protocol_id.into(),
            target_protocol_id: target_protocol_id.into(),
            target_runner_hint: None,
            pool_id: "default".into(),
            priority: 0,
            policy: "single".into(),
            metadata: BTreeMap::new(),
        }
    }

    pub fn from_protocols<P, T>(binding_id: impl Into<String>, plugin_id: impl Into<String>) -> Self
    where
        P: SdkProtocol,
        T: SdkProtocol,
    {
        Self::new(binding_id, plugin_id, P::PROTOCOL_ID, T::PROTOCOL_ID)
    }

    pub fn target_runner_hint(mut self, runner_hint: impl Into<String>) -> Self {
        self.target_runner_hint = Some(runner_hint.into());
        self
    }

    pub fn pool_id(mut self, pool_id: impl Into<String>) -> Self {
        self.pool_id = pool_id.into();
        self
    }

    pub fn priority(mut self, priority: i64) -> Self {
        self.priority = priority;
        self
    }

    pub fn policy(mut self, policy: impl Into<String>) -> Self {
        self.policy = policy.into();
        self
    }

    pub fn metadata(mut self, key: impl Into<String>, value: ScalarValue) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    pub fn build(self) -> HandlerBinding {
        HandlerBinding {
            binding_id: self.binding_id,
            plugin_id: self.plugin_id,
            protocol_id: self.protocol_id,
            target_protocol_id: self.target_protocol_id,
            target_runner_hint: self.target_runner_hint,
            pool_id: self.pool_id,
            priority: self.priority,
            policy: self.policy,
            metadata: self.metadata,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ResourceTypeDescriptorBuilder {
    kind_id: String,
    semantic: ResourceSemantic,
    schema: String,
    provider_id: String,
    operations: Vec<String>,
    reload_policy: ResourceProviderReloadPolicy,
}

impl ResourceTypeDescriptorBuilder {
    pub fn new(kind_id: impl Into<String>, semantic: ResourceSemantic) -> Self {
        let kind_id = kind_id.into();
        Self {
            schema: format!("{kind_id}.v1"),
            kind_id,
            semantic,
            provider_id: "mutsuki.resource.provider".into(),
            operations: Vec::new(),
            reload_policy: ResourceProviderReloadPolicy::CompatibleWithoutLeases,
        }
    }

    pub fn schema(mut self, schema: impl Into<String>) -> Self {
        self.schema = schema.into();
        self
    }

    pub fn provider_id(mut self, provider_id: impl Into<String>) -> Self {
        self.provider_id = provider_id.into();
        self
    }

    pub fn operations<I, S>(mut self, operations: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.operations = operations.into_iter().map(Into::into).collect();
        self
    }

    pub fn reload_policy(mut self, reload_policy: ResourceProviderReloadPolicy) -> Self {
        self.reload_policy = reload_policy;
        self
    }

    pub fn build(self) -> ResourceTypeDescriptor {
        let compatibility = ResourceProviderCompatibility {
            schema_version: self.schema.clone(),
            required_operations: self.operations.clone(),
            preserves_resource_type_id: true,
            accepts_older_generations: false,
            lease_drain_required: false,
        };
        ResourceTypeDescriptor {
            kind_id: self.kind_id,
            semantic: self.semantic,
            schema: self.schema,
            provider_id: self.provider_id,
            operations: self.operations,
            reload_policy: self.reload_policy,
            compatibility,
        }
    }
}
