pub const PLUGIN_ID: &str = "mutsuki-claude-core";
pub const RUNNER_ID: &str = "mutsuki-claude-core.claude-runner";
pub const TASK_KIND: &str = "effect.claude.run";
pub const RESULT_EVENT_KIND: &str = "claude.runner.result";

pub fn runner_surface() -> String {
    format!("runner:{RUNNER_ID}")
}

pub fn task_surface() -> String {
    format!("task_kind:{TASK_KIND}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_runtime_routes_claude_agent() {
        assert_eq!(PLUGIN_ID, "mutsuki-claude-core");
        assert_eq!(RUNNER_ID, "mutsuki-claude-core.claude-runner");
        assert_eq!(TASK_KIND, "effect.claude.run");
        assert_eq!(RESULT_EVENT_KIND, "claude.runner.result");
        assert_eq!(runner_surface(), "runner:mutsuki-claude-core.claude-runner");
        assert_eq!(task_surface(), "task_kind:effect.claude.run");
    }
}
