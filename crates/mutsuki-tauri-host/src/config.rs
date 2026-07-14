use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostMode {
    #[default]
    Embedded,
    ConnectService,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathsConfig {
    pub app_data_dir: PathBuf,
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub plugins_dir: PathBuf,
    pub resources_dir: PathBuf,
    pub runners_dir: PathBuf,
}

impl PathsConfig {
    pub fn for_app(app_name: &str) -> Self {
        let base = dirs::data_dir()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
            .join(app_name)
            .join("mutsuki");
        Self {
            app_data_dir: base.clone(),
            config_dir: base.join("config"),
            data_dir: base.join("data"),
            cache_dir: base.join("cache"),
            logs_dir: base.join("logs"),
            plugins_dir: base.join("plugins"),
            resources_dir: base.join("resources"),
            runners_dir: base.join("runners"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityConfig {
    pub require_approval_for_side_effect: bool,
    pub allow_dev_commands: bool,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            require_approval_for_side_effect: true,
            allow_dev_commands: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutsukiTauriConfig {
    pub app_name: String,
    pub app_id: String,
    pub profile_id: String,
    pub mode: HostMode,
    pub max_ticks_per_call: usize,
    pub event_buffer: usize,
    pub preview_ttl_secs: u64,
    pub paths: PathsConfig,
    pub security: SecurityConfig,
}

impl MutsukiTauriConfig {
    pub fn for_app(app_name: impl Into<String>) -> Self {
        let app_name = app_name.into();
        Self {
            app_id: format!("local.{app_name}"),
            profile_id: "default".into(),
            mode: HostMode::Embedded,
            max_ticks_per_call: 64,
            event_buffer: 1024,
            preview_ttl_secs: 300,
            paths: PathsConfig::for_app(&app_name),
            security: SecurityConfig::default(),
            app_name,
        }
    }
}
