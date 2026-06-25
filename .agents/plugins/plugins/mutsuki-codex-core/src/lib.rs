pub const PLUGIN_ID: &str = "mutsuki-codex-core";
pub const RUNNER_ID: &str = "mutsuki-codex-core.codex-runner";
pub const TASK_KIND: &str = "effect.codex.run";
pub const RESULT_EVENT_KIND: &str = "codex.strategy.result";

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
    fn codex_runner_declares_effect_task_surface() {
        assert_eq!(PLUGIN_ID, "mutsuki-codex-core");
        assert_eq!(RUNNER_ID, "mutsuki-codex-core.codex-runner");
        assert_eq!(TASK_KIND, "effect.codex.run");
        assert_eq!(RESULT_EVENT_KIND, "codex.strategy.result");
        assert_eq!(runner_surface(), "runner:mutsuki-codex-core.codex-runner");
        assert_eq!(task_surface(), "task_kind:effect.codex.run");
    }
}
