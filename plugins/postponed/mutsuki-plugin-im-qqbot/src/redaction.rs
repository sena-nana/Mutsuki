use serde_json::Value;

pub fn redact_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut redacted = serde_json::Map::new();
            for (key, item) in map {
                let lowered = key.to_ascii_lowercase();
                if lowered.contains("secret")
                    || lowered.contains("token")
                    || lowered.contains("authorization")
                {
                    redacted.insert(key.clone(), Value::String("<redacted>".into()));
                } else if lowered.contains("openid") && !matches!(item, Value::Null) {
                    redacted.insert(key.clone(), Value::String("<openid:redacted>".into()));
                } else {
                    redacted.insert(key.clone(), redact_json(item));
                }
            }
            Value::Object(redacted)
        }
        Value::Array(items) => Value::Array(items.iter().map(redact_json).collect()),
        _ => value.clone(),
    }
}
