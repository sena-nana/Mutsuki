//! Console information architecture. The product owns which activities exist; extensions
//! only contribute pages and navigation into them.

use std::collections::BTreeSet;

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityPosition {
    Top,
    Bottom,
}

/// One activity-bar entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ConsoleActivity {
    pub id: String,
    pub label: String,
    /// `@mutsuki/ui` icon name.
    pub icon: String,
    pub order: u32,
    pub position: ActivityPosition,
}

impl ConsoleActivity {
    pub fn top(id: &str, label: &str, icon: &str, order: u32) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: icon.into(),
            order,
            position: ActivityPosition::Top,
        }
    }

    pub fn bottom(id: &str, label: &str, icon: &str, order: u32) -> Self {
        Self {
            position: ActivityPosition::Bottom,
            ..Self::top(id, label, icon, order)
        }
    }
}

/// The console's activities. Extensions file navigation under these ids and cannot add more;
/// navigation for an undefined activity is reported as a layout failure in the shell.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ConsoleLayout {
    pub activities: Vec<ConsoleActivity>,
}

/// Activity hosting the shell's own settings page; every layout must define it.
pub const SETTINGS_ACTIVITY: &str = "settings";

impl Default for ConsoleLayout {
    /// Generic console: overview, plugin hubs, data, diagnostics and settings. Activities without
    /// accessible navigation are hidden, so unused entries cost nothing.
    fn default() -> Self {
        Self {
            activities: vec![
                ConsoleActivity::top("home", "概览", "home", 0),
                ConsoleActivity::top("plugins", "插件", "config", 40),
                ConsoleActivity::top("data", "数据", "data", 45),
                ConsoleActivity::top("system", "诊断", "system", 50),
                ConsoleActivity::bottom(SETTINGS_ACTIVITY, "设置", "settings", 100),
            ],
        }
    }
}

impl ConsoleLayout {
    /// Ids are unique and the settings activity exists.
    ///
    /// # Errors
    ///
    /// Names the first inconsistency.
    pub fn validate(&self) -> Result<(), String> {
        let mut ids = BTreeSet::new();
        for activity in &self.activities {
            if !ids.insert(activity.id.as_str()) {
                return Err(format!("duplicate console activity `{}`", activity.id));
            }
        }
        if !ids.contains(SETTINGS_ACTIVITY) {
            return Err(format!(
                "console layout lacks the `{SETTINGS_ACTIVITY}` activity"
            ));
        }
        Ok(())
    }

    pub fn has_activity(&self, id: &str) -> bool {
        self.activities.iter().any(|activity| activity.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_is_valid_and_serializes_positions_in_snake_case() {
        let layout = ConsoleLayout::default();
        layout.validate().unwrap();
        let json = serde_json::to_value(&layout).unwrap();
        assert_eq!(json["activities"][4]["position"], "bottom");
        assert_eq!(json["activities"][0]["position"], "top");
    }

    #[test]
    fn validation_rejects_duplicates_and_missing_settings() {
        let mut layout = ConsoleLayout::default();
        layout
            .activities
            .push(ConsoleActivity::top("home", "重复", "home", 1));
        assert!(layout.validate().unwrap_err().contains("duplicate"));

        let mut layout = ConsoleLayout::default();
        layout
            .activities
            .retain(|activity| activity.id != SETTINGS_ACTIVITY);
        assert!(layout.validate().unwrap_err().contains("settings"));
    }
}
