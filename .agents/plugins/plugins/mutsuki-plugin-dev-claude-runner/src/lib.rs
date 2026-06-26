pub const PLUGIN_ID: &str = "mutsuki.experimental.dev.claude_runner";
pub const RUNNER_ID: &str = "mutsuki.dev.claude.runner";
pub const PROTOCOL_ID: &str = "mutsuki.dev.claude.run";
pub const RESULT_EVENT_KIND: &str = "mutsuki.dev.claude.result";

pub fn runner_surface() -> String {
    format!("runner:{RUNNER_ID}")
}

pub fn protocol_surface() -> String {
    format!("task_protocol:{PROTOCOL_ID}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_runtime_routes_claude_agent() {
        assert_eq!(PLUGIN_ID, "mutsuki.experimental.dev.claude_runner");
        assert_eq!(RUNNER_ID, "mutsuki.dev.claude.runner");
        assert_eq!(PROTOCOL_ID, "mutsuki.dev.claude.run");
        assert_eq!(RESULT_EVENT_KIND, "mutsuki.dev.claude.result");
        assert_eq!(runner_surface(), "runner:mutsuki.dev.claude.runner");
        assert_eq!(protocol_surface(), "task_protocol:mutsuki.dev.claude.run");
    }
}
