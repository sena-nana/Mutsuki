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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "layout", content = "payload", rename_all = "snake_case")]
pub enum BatchPayload {
    Row(RowPayload),
    Columnar(ColumnarPayload),
    BinaryPacked(BinaryPackedPayload),
    ResourceBacked(ResourceBackedPayload),
}

impl BatchPayload {
    pub fn from_tasks(tasks: &[Task]) -> Self {
        Self::from_task_refs(tasks.iter())
    }

    pub fn from_task_refs<'a>(tasks: impl IntoIterator<Item = &'a Task>) -> Self {
        Self::Row(RowPayload {
            rows: tasks
                .into_iter()
                .map(|task| serde_json::to_value(task).expect("Task serializes"))
                .collect(),
        })
    }

    pub fn layout(&self) -> PayloadLayout {
        match self {
            Self::Row(_) => PayloadLayout::Row,
            Self::Columnar(_) => PayloadLayout::Columnar,
            Self::BinaryPacked(_) => PayloadLayout::BinaryPacked,
            Self::ResourceBacked(_) => PayloadLayout::ResourceBacked,
        }
    }

    pub fn row_count(&self) -> usize {
        match self {
            Self::Row(payload) => payload.rows.len(),
            Self::Columnar(payload) => payload.row_count,
            Self::BinaryPacked(payload) => payload.row_count,
            Self::ResourceBacked(payload) => payload.slices.len(),
        }
    }

    // RuntimeError is the stable, structured wire error; boxing it would change the public API.
    #[allow(clippy::result_large_err)]
    pub fn try_row_tasks(&self) -> Result<Vec<Task>, RuntimeError> {
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
}

fn payload_error(route: String) -> RuntimeError {
    RuntimeError::new(ERR_TASK_CLAIM_CONFLICT, "runtime.batch_payload", route)
}
