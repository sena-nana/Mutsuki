use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub use mutsuki_agent_contracts::{
    CredentialRef, ModelGenerateRequest, ModelGenerateResult, ModelProtocolAdapterDescriptor,
    ModelSelector, ModelStreamEvent, ProtocolError, ProviderInstanceDescriptor,
};

pub type ModelAdapterFuture =
    Pin<Box<dyn Future<Output = Result<ModelGenerateResult, ProtocolError>> + Send + 'static>>;
pub type ModelStreamFuture =
    Pin<Box<dyn Future<Output = Result<Vec<ModelStreamEvent>, ProtocolError>> + Send + 'static>>;
pub type CredentialFuture =
    Pin<Box<dyn Future<Output = Result<CredentialValue, ProtocolError>> + Send + 'static>>;

#[derive(Clone)]
pub struct CredentialValue(Arc<str>);

impl CredentialValue {
    pub fn new(value: impl Into<String>) -> Result<Self, ProtocolError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(config_error("credential value is empty"));
        }
        Ok(Self(Arc::from(value)))
    }

    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for CredentialValue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CredentialValue(<redacted>)")
    }
}

pub trait CredentialBroker: Send + Sync {
    fn resolve(&self, credential: CredentialRef) -> CredentialFuture;
}

pub trait ModelProtocolAdapter: Send + Sync {
    fn descriptor(&self) -> &ModelProtocolAdapterDescriptor;

    fn generate(
        &self,
        provider: ProviderInstanceDescriptor,
        request: ModelGenerateRequest,
    ) -> ModelAdapterFuture;

    fn stream(
        &self,
        _provider: ProviderInstanceDescriptor,
        _request: ModelGenerateRequest,
    ) -> ModelStreamFuture {
        Box::pin(async {
            Err(ProtocolError {
                code: "agent.adapter.streaming_unsupported".into(),
                class: mutsuki_agent_contracts::ProtocolErrorClass::NonRetryable,
                message: "adapter does not support streaming".into(),
                retry_after_ms: None,
            })
        })
    }
}

pub type TranscriptionFuture = Pin<
    Box<
        dyn Future<Output = Result<mutsuki_agent_contracts::TranscriptionResult, ProtocolError>>
            + Send
            + 'static,
    >,
>;
pub type SpeechSynthesisFuture = Pin<
    Box<
        dyn Future<Output = Result<mutsuki_agent_contracts::SpeechSynthesisResult, ProtocolError>>
            + Send
            + 'static,
    >,
>;
pub type TranscriptionStreamFuture = Pin<
    Box<
        dyn Future<Output = Result<Vec<mutsuki_agent_contracts::TranscriptionEvent>, ProtocolError>>
            + Send
            + 'static,
    >,
>;
pub type SpeechStreamFuture = Pin<
    Box<
        dyn Future<
                Output = Result<Vec<mutsuki_agent_contracts::SpeechSynthesisEvent>, ProtocolError>,
            > + Send
            + 'static,
    >,
>;

/// Protocol-level STT/TTS surface. Brand-specific providers remain Host configuration.
pub trait MediaProtocolAdapter: Send + Sync {
    fn descriptor(&self) -> &ModelProtocolAdapterDescriptor;

    fn transcribe(
        &self,
        provider: ProviderInstanceDescriptor,
        request: mutsuki_agent_contracts::TranscriptionRequest,
    ) -> TranscriptionFuture;

    fn synthesize(
        &self,
        provider: ProviderInstanceDescriptor,
        request: mutsuki_agent_contracts::SpeechSynthesisRequest,
    ) -> SpeechSynthesisFuture;

    fn transcribe_stream(
        &self,
        provider: ProviderInstanceDescriptor,
        request: mutsuki_agent_contracts::TranscriptionRequest,
    ) -> TranscriptionStreamFuture {
        let future = self.transcribe(provider, request);
        Box::pin(async move {
            let result = future.await?;
            Ok(vec![
                mutsuki_agent_contracts::TranscriptionEvent::Completed {
                    sequence: 1,
                    result: Box::new(result),
                },
            ])
        })
    }

    fn synthesize_stream(
        &self,
        provider: ProviderInstanceDescriptor,
        request: mutsuki_agent_contracts::SpeechSynthesisRequest,
    ) -> SpeechStreamFuture {
        let future = self.synthesize(provider, request);
        Box::pin(async move {
            let result = future.await?;
            Ok(vec![
                mutsuki_agent_contracts::SpeechSynthesisEvent::Completed {
                    sequence: 1,
                    result,
                },
            ])
        })
    }
}

#[derive(Clone, Default)]
pub struct ModelAdapterCatalog {
    adapters: Arc<BTreeMap<String, Arc<dyn ModelProtocolAdapter>>>,
    providers: Arc<BTreeMap<String, ProviderInstanceDescriptor>>,
}

impl ModelAdapterCatalog {
    pub fn build(
        adapters: impl IntoIterator<Item = Arc<dyn ModelProtocolAdapter>>,
        providers: impl IntoIterator<Item = ProviderInstanceDescriptor>,
    ) -> Result<Self, ProtocolError> {
        let mut adapter_map = BTreeMap::new();
        for adapter in adapters {
            let id = adapter.descriptor().adapter_id.clone();
            if id.trim().is_empty() || adapter_map.insert(id.clone(), adapter).is_some() {
                return Err(config_error(format!(
                    "duplicate or empty adapter id `{id}`"
                )));
            }
        }
        let mut provider_map = BTreeMap::new();
        for provider in providers {
            if !adapter_map.contains_key(&provider.adapter_id) {
                return Err(config_error(format!(
                    "provider `{}` references unknown adapter `{}`",
                    provider.provider_id, provider.adapter_id
                )));
            }
            let id = provider.provider_id.clone();
            if id.trim().is_empty() || provider_map.insert(id.clone(), provider).is_some() {
                return Err(config_error(format!(
                    "duplicate or empty provider id `{id}`"
                )));
            }
        }
        Ok(Self {
            adapters: Arc::new(adapter_map),
            providers: Arc::new(provider_map),
        })
    }

    pub fn select(
        &self,
        selector: &ModelSelector,
    ) -> Result<(Arc<dyn ModelProtocolAdapter>, ProviderInstanceDescriptor), ProtocolError> {
        let provider = self
            .providers
            .values()
            .find(|provider| {
                (selector.allowed_provider_ids.is_empty()
                    || selector
                        .allowed_provider_ids
                        .contains(&provider.provider_id))
                    && (selector.allowed_adapter_ids.is_empty()
                        || selector.allowed_adapter_ids.contains(&provider.adapter_id))
                    && selector
                        .preferred_model
                        .as_ref()
                        .is_none_or(|model| provider.models.contains_key(model))
            })
            .cloned()
            .ok_or_else(|| config_error("no provider matches the model selector"))?;
        let adapter = self
            .adapters
            .get(&provider.adapter_id)
            .cloned()
            .ok_or_else(|| config_error("selected provider adapter is unavailable"))?;
        Ok((adapter, provider))
    }
}

fn config_error(message: impl Into<String>) -> ProtocolError {
    ProtocolError {
        code: "agent.adapter.invalid_config".into(),
        class: mutsuki_agent_contracts::ProtocolErrorClass::NonRetryable,
        message: message.into(),
        retry_after_ms: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_agent_contracts::{CredentialRef, ModelCapability, ProtocolErrorClass};

    struct TestAdapter {
        descriptor: ModelProtocolAdapterDescriptor,
    }

    impl ModelProtocolAdapter for TestAdapter {
        fn descriptor(&self) -> &ModelProtocolAdapterDescriptor {
            &self.descriptor
        }

        fn generate(
            &self,
            _provider: ProviderInstanceDescriptor,
            _request: ModelGenerateRequest,
        ) -> ModelAdapterFuture {
            Box::pin(async {
                Err(ProtocolError {
                    code: "not-called".into(),
                    class: ProtocolErrorClass::NonRetryable,
                    message: "selection test".into(),
                    retry_after_ms: None,
                })
            })
        }
    }

    #[test]
    fn catalog_selects_protocol_adapter_without_provider_brand_branching() {
        let adapter: Arc<dyn ModelProtocolAdapter> = Arc::new(TestAdapter {
            descriptor: ModelProtocolAdapterDescriptor {
                adapter_id: "openai-compatible".into(),
                protocol: "openai.chat-completions".into(),
                version: "1".into(),
                runner_id: "agent.adapter.openai-compatible".into(),
                capability: ModelCapability::default(),
            },
        });
        let providers =
            ["provider-a", "provider-b"].map(|provider_id| ProviderInstanceDescriptor {
                provider_id: provider_id.into(),
                adapter_id: "openai-compatible".into(),
                endpoint: format!("https://{provider_id}.invalid/v1"),
                credential: CredentialRef {
                    credential_id: format!("{provider_id}-credential"),
                    revision: 1,
                },
                models: BTreeMap::from([("model".into(), ModelCapability::default())]),
                headers: BTreeMap::new(),
                compatibility: BTreeMap::new(),
                remote_execution_allowed: true,
            });
        let catalog = ModelAdapterCatalog::build([adapter], providers).unwrap();
        let (_, provider) = catalog
            .select(&ModelSelector {
                preferred_model: Some("model".into()),
                allowed_provider_ids: vec!["provider-b".into()],
                ..ModelSelector::default()
            })
            .unwrap();
        assert_eq!(provider.provider_id, "provider-b");
    }
}
