use std::borrow::Borrow;
use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! string_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_string(self) -> String {
                self.0
            }

            pub fn is_empty(&self) -> bool {
                self.0.is_empty()
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl From<&String> for $name {
            fn from(value: &String) -> Self {
                Self(value.clone())
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl From<&$name> for $name {
            fn from(value: &$name) -> Self {
                value.clone()
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl PartialEq<str> for $name {
            fn eq(&self, other: &str) -> bool {
                self.0 == other
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.0 == *other
            }
        }

        impl PartialEq<String> for $name {
            fn eq(&self, other: &String) -> bool {
                &self.0 == other
            }
        }
    };
}

string_id!(
    /// Resource / value descriptor identity.
    RefId
);
string_id!(
    /// Task identity.
    TaskId
);
string_id!(
    /// Logical runner identity.
    RunnerId
);
string_id!(
    /// Plugin identity.
    PluginId
);
string_id!(
    /// Physical executor identity.
    ExecutorId
);
string_id!(
    /// Handler binding identity.
    BindingId
);
string_id!(
    /// Protocol identity.
    ProtocolId
);
string_id!(
    /// Task execution lease identity.
    TaskLeaseId
);
string_id!(
    /// Scheduler tick identity.
    TickId
);
string_id!(
    /// Work / submit batch identity.
    BatchId
);
string_id!(
    /// Batch entry identity.
    EntryId
);
string_id!(
    /// Batch grouping key.
    BatchKey
);
string_id!(
    /// Long-lived resource cell identity.
    ResourceCellId
);
string_id!(
    /// Resource-cell lease identity.
    ResourceLeaseId
);
string_id!(
    /// Public surface identity.
    SurfaceId
);
string_id!(
    /// Trace span identity.
    SpanId
);
string_id!(
    /// Trace identity.
    TraceId
);
string_id!(
    /// Capability request identity used for correlation and idempotent replay.
    CapabilityRequestId
);
string_id!(
    /// Host-owned opaque capability peer identity.
    CapabilityPeerId
);

pub type PayloadIndex = usize;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScalarValue {
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
}

#[cfg(test)]
mod tests {
    use super::{RefId, TaskId};

    #[test]
    fn string_ids_round_trip_as_json_strings() {
        let task_id = TaskId::from("task-1");
        let encoded = serde_json::to_value(&task_id).unwrap();
        assert_eq!(encoded, serde_json::json!("task-1"));
        assert_eq!(
            serde_json::from_value::<TaskId>(encoded).unwrap(),
            TaskId::from("task-1")
        );
    }

    #[test]
    fn string_ids_compare_with_str_but_not_each_other() {
        let task_id = TaskId::from("shared");
        let ref_id = RefId::from("shared");
        assert_eq!(task_id, "shared");
        assert_eq!(ref_id, "shared");
        assert_eq!(task_id.as_str(), ref_id.as_str());
        let _: TaskId = "task-1".into();
        let _: RefId = String::from("resource:1").into();
    }
}
