//! Thin helpers over RuntimeClient / TaskHandle. No local scheduler.

use mutsuki_runtime_sdk::contracts::Task;

/// Attach `trace_id` / `correlation_id` before submit via RuntimeClient / TaskSubmitter.
pub fn with_trace(
    mut task: Task,
    trace_id: Option<String>,
    correlation_id: Option<String>,
) -> Task {
    task.trace_id = trace_id;
    task.correlation_id = correlation_id;
    task
}

#[cfg(test)]
mod tests {
    use mutsuki_runtime_sdk::contracts::Task;

    use super::with_trace;

    #[test]
    fn with_trace_sets_ids() {
        let task = with_trace(
            Task::new("t", "mutsuki.agent/run@1", serde_json::json!({})),
            Some("tr".into()),
            Some("co".into()),
        );
        assert_eq!(task.trace_id.as_deref(), Some("tr"));
        assert_eq!(task.correlation_id.as_deref(), Some("co"));
    }
}
