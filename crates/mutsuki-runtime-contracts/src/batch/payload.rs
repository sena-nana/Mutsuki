use std::borrow::Cow;
use std::sync::Arc;

use serde::ser::SerializeSeq;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ERR_TASK_CLAIM_CONFLICT, ResourceRef, RuntimeError, Task};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PayloadLayout {
    Row,
    Columnar,
    BinaryPacked,
    ResourceBacked,
}

impl PayloadLayout {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Row => "row",
            Self::Columnar => "columnar",
            Self::BinaryPacked => "binary_packed",
            Self::ResourceBacked => "resource_backed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RowPayload {
    pub rows: Vec<Value>,
}

/// In-process task payload used by builtin runners.
///
/// Serialization deliberately preserves the existing row wire shape, so the
/// typed representation never becomes a new ABI or persistence format.
#[derive(Clone, Debug, PartialEq)]
pub struct LocalTaskPayload {
    pub tasks: Vec<Arc<Task>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColumnarPayload {
    pub columns: Vec<ColumnPayload>,
    pub row_count: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColumnPayload {
    pub name: String,
    pub values: Vec<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BinaryPackedPayload {
    pub encoding: String,
    pub bytes: Vec<u8>,
    pub row_count: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResourceSlice {
    pub resource: ResourceRef,
    pub offset: u64,
    pub length: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResourceBackedPayload {
    pub slices: Vec<ResourceSlice>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BatchPayload {
    Local(LocalTaskPayload),
    Row(RowPayload),
    Columnar(ColumnarPayload),
    BinaryPacked(BinaryPackedPayload),
    ResourceBacked(ResourceBackedPayload),
}

impl BatchPayload {
    pub fn from_local_tasks<T>(tasks: Vec<T>) -> Self
    where
        T: Into<Arc<Task>>,
    {
        Self::Local(LocalTaskPayload {
            tasks: tasks.into_iter().map(Into::into).collect(),
        })
    }

    pub fn from_tasks(tasks: &[Task]) -> Self {
        Self::from_task_refs(tasks.iter())
    }

    /// Builds an in-process local payload that shares each task by `Arc`.
    ///
    /// Prefer this over JSON row materialization on builtin hot paths. Wire
    /// serialization still projects to the existing row layout.
    pub fn from_task_refs<'a>(tasks: impl IntoIterator<Item = &'a Task>) -> Self {
        Self::Local(LocalTaskPayload {
            tasks: tasks
                .into_iter()
                .map(|task| Arc::new(task.clone()))
                .collect(),
        })
    }

    /// Wire-oriented row payload that eagerly serializes each task to JSON.
    pub fn from_tasks_json(tasks: &[Task]) -> Self {
        Self::Row(RowPayload {
            rows: tasks
                .iter()
                .map(|task| serde_json::to_value(task).expect("Task serializes"))
                .collect(),
        })
    }

    pub fn layout(&self) -> PayloadLayout {
        match self {
            Self::Local(_) => PayloadLayout::Row,
            Self::Row(_) => PayloadLayout::Row,
            Self::Columnar(_) => PayloadLayout::Columnar,
            Self::BinaryPacked(_) => PayloadLayout::BinaryPacked,
            Self::ResourceBacked(_) => PayloadLayout::ResourceBacked,
        }
    }

    pub fn row_count(&self) -> usize {
        match self {
            Self::Local(payload) => payload.tasks.len(),
            Self::Row(payload) => payload.rows.len(),
            Self::Columnar(payload) => payload.row_count,
            Self::BinaryPacked(payload) => payload.row_count,
            Self::ResourceBacked(payload) => payload.slices.len(),
        }
    }

    // RuntimeError is the stable, structured wire error; boxing it would change the public API.
    #[allow(clippy::result_large_err)]
    pub fn try_row_tasks(&self) -> Result<Vec<Task>, RuntimeError> {
        if let Self::Local(payload) = self {
            return Ok(payload
                .tasks
                .iter()
                .map(|task| task.as_ref().clone())
                .collect());
        }
        let Self::Row(payload) = self else {
            return Err(payload_error(format!(
                "payload.layout.{}",
                self.layout().as_str()
            )));
        };
        payload
            .rows
            .iter()
            .enumerate()
            .map(|(index, value)| {
                serde_json::from_value(value.clone())
                    .map_err(|_| payload_error(format!("payload.row.{index}")))
            })
            .collect()
    }

    #[allow(clippy::result_large_err)]
    pub fn task_at(&self, index: usize) -> Result<Cow<'_, Task>, RuntimeError> {
        match self {
            Self::Local(payload) => payload
                .tasks
                .get(index)
                .map(|task| Cow::Borrowed(task.as_ref()))
                .ok_or_else(|| payload_error(format!("payload.row.{index}.missing"))),
            Self::Row(payload) => payload
                .rows
                .get(index)
                .ok_or_else(|| payload_error(format!("payload.row.{index}.missing")))
                .and_then(|value| {
                    serde_json::from_value(value.clone())
                        .map(Cow::Owned)
                        .map_err(|_| payload_error(format!("payload.row.{index}")))
                }),
            _ => Err(payload_error(format!(
                "payload.layout.{}",
                self.layout().as_str()
            ))),
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "layout", content = "payload", rename_all = "snake_case")]
enum BatchPayloadRef<'a> {
    Row(RowPayloadRef<'a>),
    Columnar(&'a ColumnarPayload),
    BinaryPacked(&'a BinaryPackedPayload),
    ResourceBacked(&'a ResourceBackedPayload),
}

#[derive(Serialize)]
struct RowPayloadRef<'a> {
    rows: RowItemsRef<'a>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum RowItemsRef<'a> {
    Values(&'a [Value]),
    Tasks(LocalTasksRef<'a>),
}

struct LocalTasksRef<'a>(&'a [Arc<Task>]);

impl Serialize for LocalTasksRef<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for task in self.0 {
            sequence.serialize_element(task.as_ref())?;
        }
        sequence.end()
    }
}

impl Serialize for BatchPayload {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Local(payload) => BatchPayloadRef::Row(RowPayloadRef {
                rows: RowItemsRef::Tasks(LocalTasksRef(&payload.tasks)),
            })
            .serialize(serializer),
            Self::Row(payload) => BatchPayloadRef::Row(RowPayloadRef {
                rows: RowItemsRef::Values(&payload.rows),
            })
            .serialize(serializer),
            Self::Columnar(payload) => BatchPayloadRef::Columnar(payload).serialize(serializer),
            Self::BinaryPacked(payload) => {
                BatchPayloadRef::BinaryPacked(payload).serialize(serializer)
            }
            Self::ResourceBacked(payload) => {
                BatchPayloadRef::ResourceBacked(payload).serialize(serializer)
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "layout", content = "payload", rename_all = "snake_case")]
enum WireBatchPayload {
    Row(RowPayload),
    Columnar(ColumnarPayload),
    BinaryPacked(BinaryPackedPayload),
    ResourceBacked(ResourceBackedPayload),
}

impl<'de> Deserialize<'de> for BatchPayload {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(match WireBatchPayload::deserialize(deserializer)? {
            WireBatchPayload::Row(payload) => Self::Row(payload),
            WireBatchPayload::Columnar(payload) => Self::Columnar(payload),
            WireBatchPayload::BinaryPacked(payload) => Self::BinaryPacked(payload),
            WireBatchPayload::ResourceBacked(payload) => Self::ResourceBacked(payload),
        })
    }
}

fn payload_error(route: String) -> RuntimeError {
    RuntimeError::new(ERR_TASK_CLAIM_CONFLICT, "runtime.batch_payload", route)
}
