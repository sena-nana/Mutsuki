//! Typed RPC and event contracts.
//!
//! An owner declares each method once as a [`TypedRpc`] marker; the WebExtension registers it
//! with `RpcRegistry::register_typed` and the frontend contract module is generated from the
//! same markers by [`ts_contract_module`], so namespace, method name, operation and capability
//! cannot drift between Rust and TypeScript.

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Whether a call may be repeated and cached (`Read`) or must never be repeated implicitly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RpcOperation {
    Read,
    Write,
}

/// One RPC method of a WebExtension namespace.
pub trait TypedRpc {
    const NAMESPACE: &'static str;
    const METHOD: &'static str;
    const OPERATION: RpcOperation;
    /// Capability the Host enforces before the handler runs.
    const CAPABILITY: &'static str;
    /// TypeScript type names of [`Self::Params`] and [`Self::Result`] in the generated module.
    const TS_PARAMS: &'static str;
    const TS_RESULT: &'static str;
    type Params: Serialize + DeserializeOwned + Send + 'static;
    type Result: Serialize + DeserializeOwned + Send + 'static;
}

/// One event topic of a WebExtension namespace.
pub trait TypedTopic {
    const NAMESPACE: &'static str;
    const TOPIC: &'static str;
    const CAPABILITY: &'static str;
    const TS_PAYLOAD: &'static str;
    type Payload: Serialize + DeserializeOwned + Send + 'static;
}

/// Static description of a [`TypedRpc`] used to generate frontend contracts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RpcMethodDescriptor {
    pub method: &'static str,
    pub operation: RpcOperation,
    pub capability: &'static str,
    pub ts_params: &'static str,
    pub ts_result: &'static str,
}

impl RpcMethodDescriptor {
    #[must_use]
    pub fn of<M: TypedRpc>() -> Self {
        Self {
            method: M::METHOD,
            operation: M::OPERATION,
            capability: M::CAPABILITY,
            ts_params: M::TS_PARAMS,
            ts_result: M::TS_RESULT,
        }
    }
}

/// Static description of a [`TypedTopic`] used to generate frontend contracts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopicDescriptor {
    pub topic: &'static str,
    pub capability: &'static str,
    pub ts_payload: &'static str,
}

impl TopicDescriptor {
    #[must_use]
    pub fn of<T: TypedTopic>() -> Self {
        Self {
            topic: T::TOPIC,
            capability: T::CAPABILITY,
            ts_payload: T::TS_PAYLOAD,
        }
    }
}

/// Renders a TypeScript module exporting `export const {api_name} = defineRpc(..)` and one
/// `topic(..)` constant per event, importing the payload types from `types_module`.
///
/// The output is deterministic; owners commit it and a test fails when it is stale.
#[must_use]
pub fn ts_contract_module(
    api_name: &str,
    namespace: &str,
    types_module: &str,
    methods: &[RpcMethodDescriptor],
    topics: &[TopicDescriptor],
) -> String {
    let mut types = methods
        .iter()
        .flat_map(|method| [method.ts_params, method.ts_result])
        .chain(topics.iter().map(|topic| topic.ts_payload))
        .flat_map(ts_identifiers)
        .collect::<Vec<_>>();
    types.sort_unstable();
    types.dedup();
    let mut out = String::from(
        "// Generated from the Rust contract markers; do not edit.\n\
         // Regenerate with MUTSUKI_BLESS_TS_CONTRACT=1 cargo test.\n",
    );
    let sdk_imports = [
        "defineRpc",
        if methods.iter().any(|m| m.operation == RpcOperation::Write) {
            "mutation"
        } else {
            ""
        },
        if methods.iter().any(|m| m.operation == RpcOperation::Read) {
            "query"
        } else {
            ""
        },
        if topics.is_empty() { "" } else { "topic" },
    ]
    .into_iter()
    .filter(|name| !name.is_empty())
    .collect::<Vec<_>>()
    .join(", ");
    out.push_str(&format!(
        "import {{ {sdk_imports} }} from \"@mutsuki/web-sdk\";\n"
    ));
    if !types.is_empty() {
        out.push_str(&format!(
            "import type {{ {} }} from \"{types_module}\";\n",
            types.join(", ")
        ));
    }
    out.push_str(&format!(
        "\nexport const {api_name} = defineRpc({{\n  namespace: \"{namespace}\",\n  methods: {{\n"
    ));
    for method in methods {
        let builder = match method.operation {
            RpcOperation::Read => "query",
            RpcOperation::Write => "mutation",
        };
        out.push_str(&format!(
            "    \"{}\": {builder}<{}, {}>({{ capability: \"{}\" }}),\n",
            method.method, method.ts_params, method.ts_result, method.capability
        ));
    }
    out.push_str("  },\n});\n");
    for topic in topics {
        let constant = topic
            .topic
            .split(['.', '-', '_'])
            .filter(|part| !part.is_empty())
            .enumerate()
            .map(|(index, part)| {
                if index == 0 {
                    part.to_owned()
                } else {
                    let mut chars = part.chars();
                    chars
                        .next()
                        .map(|first| first.to_uppercase().chain(chars).collect())
                        .unwrap_or_default()
                }
            })
            .collect::<String>();
        out.push_str(&format!(
            "\nexport const {constant}Topic = topic<{}>(\"{namespace}.{}\", \"{}\");\n",
            topic.ts_payload, topic.topic, topic.capability
        ));
    }
    out
}

/// TypeScript identifiers in a type expression that must be imported (not builtins).
fn ts_identifiers(expression: &str) -> Vec<&str> {
    expression
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|word| word.chars().next().is_some_and(char::is_uppercase))
        .filter(|word| !matches!(*word, "Record" | "Array" | "Partial" | "Readonly"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ListThings;

    impl TypedRpc for ListThings {
        const NAMESPACE: &'static str = "things";
        const METHOD: &'static str = "things.list";
        const OPERATION: RpcOperation = RpcOperation::Read;
        const CAPABILITY: &'static str = "things.read";
        const TS_PARAMS: &'static str = "ListThingsParams";
        const TS_RESULT: &'static str = "Thing[]";
        type Params = serde_json::Value;
        type Result = Vec<serde_json::Value>;
    }

    #[test]
    fn contract_module_imports_used_types_and_declares_methods() {
        let module = ts_contract_module(
            "thingsApi",
            "things",
            "../types",
            &[RpcMethodDescriptor::of::<ListThings>()],
            &[TopicDescriptor {
                topic: "changed",
                capability: "things.read",
                ts_payload: "Record<string, never>",
            }],
        );
        assert!(module.contains("import { defineRpc, query, topic } from \"@mutsuki/web-sdk\";"));
        assert!(module.contains("import type { ListThingsParams, Thing } from \"../types\";"));
        assert!(module.contains(
            "\"things.list\": query<ListThingsParams, Thing[]>({ capability: \"things.read\" }),"
        ));
        assert!(module.contains(
            "export const changedTopic = topic<Record<string, never>>(\"things.changed\", \"things.read\");"
        ));
    }
}
