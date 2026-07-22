use mutsuki_runtime_contracts::{
    BatchPayload, BinaryPackedPayload, ColumnPayload, ColumnarPayload, OrderingRequirement,
    ResourceAccessMode, ResourceBackedPayload, ResourceRequirement, ResourceSlice, Task, TaskBatch,
    WorkResourcePlan,
};

#[derive(Clone, Debug)]
pub struct TaskBatchBuilder {
    batch_id: String,
    tick_id: Option<String>,
    tasks: Vec<Task>,
    resource_plan: Option<WorkResourcePlan>,
}

impl TaskBatchBuilder {
    pub fn new(batch_id: impl Into<String>) -> Self {
        Self {
            batch_id: batch_id.into(),
            tick_id: None,
            tasks: Vec::new(),
            resource_plan: None,
        }
    }

    pub fn tick_id(mut self, tick_id: impl Into<String>) -> Self {
        self.tick_id = Some(tick_id.into());
        self
    }

    pub fn resource_plan(mut self, plan: WorkResourcePlan) -> Self {
        self.resource_plan = Some(plan);
        self
    }

    pub fn task(mut self, task: Task) -> Self {
        self.tasks.push(task);
        self
    }

    pub fn build(self) -> TaskBatch {
        TaskBatch {
            batch_id: self.batch_id,
            tick_id: self.tick_id,
            tasks: self.tasks,
            resource_plan: self.resource_plan,
        }
    }
}

pub struct TaskOptions;

impl TaskOptions {
    pub fn read(ref_id: impl Into<String>, expected_version: Option<u64>) -> ResourceRequirement {
        ResourceRequirement {
            ref_id: ref_id.into(),
            mode: ResourceAccessMode::Read,
            expected_version,
        }
    }

    pub fn write(ref_id: impl Into<String>, expected_version: Option<u64>) -> ResourceRequirement {
        ResourceRequirement {
            ref_id: ref_id.into(),
            mode: ResourceAccessMode::Write,
            expected_version,
        }
    }

    pub fn exclusive_write(
        ref_id: impl Into<String>,
        expected_version: Option<u64>,
    ) -> ResourceRequirement {
        ResourceRequirement {
            ref_id: ref_id.into(),
            mode: ResourceAccessMode::ExclusiveWrite,
            expected_version,
        }
    }

    pub fn strict_sequence(sequence_id: impl Into<String>) -> OrderingRequirement {
        OrderingRequirement::StrictSequence {
            sequence_id: sequence_id.into(),
        }
    }
}

pub struct BatchPayloadBuilder;

impl BatchPayloadBuilder {
    pub fn row_tasks(tasks: &[Task]) -> BatchPayload {
        BatchPayload::from_tasks(tasks)
    }

    pub fn local_tasks(tasks: Vec<Task>) -> BatchPayload {
        BatchPayload::from_local_tasks(tasks)
    }

    pub fn row_tasks_json(tasks: &[Task]) -> BatchPayload {
        BatchPayload::from_tasks_json(tasks)
    }

    pub fn columnar(columns: Vec<ColumnPayload>, row_count: usize) -> BatchPayload {
        BatchPayload::Columnar(ColumnarPayload { columns, row_count })
    }

    pub fn binary_packed(
        encoding: impl Into<String>,
        bytes: Vec<u8>,
        row_count: usize,
    ) -> BatchPayload {
        BatchPayload::BinaryPacked(BinaryPackedPayload {
            encoding: encoding.into(),
            bytes,
            row_count,
        })
    }

    pub fn resource_backed(slices: Vec<ResourceSlice>) -> BatchPayload {
        BatchPayload::ResourceBacked(ResourceBackedPayload { slices })
    }
}
