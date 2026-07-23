//! Optional JSON Schema export — not the internal source of truth.

use serde_json::{Value, json};

use crate::schema::{ConfigDescriptor, ConfigNode, ConfigValueType};

pub fn to_json_schema(descriptor: &ConfigDescriptor) -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": descriptor.title.default,
        "description": descriptor.description.as_ref().map(|d| d.default.clone()),
        "x-mutsuki-provider-id": descriptor.provider_id.0,
        "x-mutsuki-schema-version": descriptor.schema_version,
        "x-mutsuki-value-version": descriptor.value_version,
        "type": "object",
        "properties": properties_from_node(&descriptor.root),
        "additionalProperties": false,
    })
}

fn properties_from_node(node: &ConfigNode) -> Value {
    let mut props = serde_json::Map::new();
    for child in &node.children {
        props.insert(child.key.as_str().to_string(), node_to_schema(child));
    }
    Value::Object(props)
}

fn node_to_schema(node: &ConfigNode) -> Value {
    let mut schema = match &node.value_type {
        ConfigValueType::Bool => json!({"type": "boolean"}),
        ConfigValueType::Integer => json!({"type": "integer"}),
        ConfigValueType::Float => json!({"type": "number"}),
        ConfigValueType::String { multiline } => json!({
            "type": "string",
            "x-mutsuki-multiline": multiline,
        }),
        ConfigValueType::Secret => json!({
            "type": "object",
            "x-mutsuki-secret": true,
        }),
        ConfigValueType::Enum { options, multi } => {
            let values: Vec<_> = options.iter().map(|o| o.value.clone()).collect();
            if *multi {
                json!({"type": "array", "items": {"type": "string", "enum": values}})
            } else {
                json!({"type": "string", "enum": values})
            }
        }
        ConfigValueType::Object => json!({
            "type": "object",
            "properties": properties_from_node(node),
            "additionalProperties": false,
        }),
        ConfigValueType::Array { item } => json!({
            "type": "array",
            "items": type_to_schema(item),
        }),
        ConfigValueType::Map { value, .. } => json!({
            "type": "object",
            "additionalProperties": type_to_schema(value),
        }),
        ConfigValueType::FileRef | ConfigValueType::DirectoryRef => json!({"type": "string"}),
        ConfigValueType::Null => json!({"type": "null"}),
    };
    if let Some(obj) = schema.as_object_mut() {
        obj.insert("title".into(), Value::String(node.title.default.clone()));
        if let Some(min) = node.constraints.min {
            obj.insert("minimum".into(), json!(min));
        }
        if let Some(max) = node.constraints.max {
            obj.insert("maximum".into(), json!(max));
        }
        if node.presentation.secret {
            obj.insert("x-mutsuki-secret".into(), Value::Bool(true));
        }
        if let Some(format) = &node.presentation.format {
            obj.insert("x-mutsuki-format".into(), Value::String(format.clone()));
        }
    }
    schema
}

fn type_to_schema(ty: &ConfigValueType) -> Value {
    match ty {
        ConfigValueType::Bool => json!({"type": "boolean"}),
        ConfigValueType::Integer => json!({"type": "integer"}),
        ConfigValueType::Float => json!({"type": "number"}),
        ConfigValueType::String { .. } => json!({"type": "string"}),
        ConfigValueType::Secret => json!({"type": "object", "x-mutsuki-secret": true}),
        other => json!({"x-mutsuki-type": format!("{other:?}")}),
    }
}
