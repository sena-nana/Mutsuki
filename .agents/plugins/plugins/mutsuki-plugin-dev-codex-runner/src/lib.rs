pub const PLUGIN_ID: &str = "mutsuki.experimental.dev.codex_runner";
pub const RUNNER_ID: &str = "mutsuki.dev.codex.runner";
pub const PROTOCOL_ID: &str = "mutsuki.dev.codex.run";
pub const RESULT_EVENT_KIND: &str = "mutsuki.dev.codex.result";

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
    fn codex_runner_declares_dev_protocol_surface() {
        assert_eq!(PLUGIN_ID, "mutsuki.experimental.dev.codex_runner");
        assert_eq!(RUNNER_ID, "mutsuki.dev.codex.runner");
        assert_eq!(PROTOCOL_ID, "mutsuki.dev.codex.run");
        assert_eq!(RESULT_EVENT_KIND, "mutsuki.dev.codex.result");
        assert_eq!(runner_surface(), "runner:mutsuki.dev.codex.runner");
        assert_eq!(protocol_surface(), "task_protocol:mutsuki.dev.codex.run");
    }
}
