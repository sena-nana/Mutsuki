use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
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

/// 平台标准数据目录，行为对齐原 `dirs::data_dir()`：
/// macOS `~/Library/Application Support`、Windows `%APPDATA%`、其余 XDG 数据目录。
fn system_data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        std::env::home_dir().map(|home| home.join("Library/Application Support"))
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(PathBuf::from)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| std::env::home_dir().map(|home| home.join(".local/share")))
    }
}

impl PathsConfig {
    pub fn for_app(app_name: &str) -> Self {
        let base = system_data_dir()
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

/// 桌面 Host 对已安装插件的启用选择与初始化配置。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginSelection {
    /// `None` 表示启用所有可执行插件；`Some` 仅启用集合中的插件。
    #[serde(default)]
    pub enabled_plugin_ids: Option<BTreeSet<String>>,
    /// 仅在 ABI 初始化边界传递，不写入解压缓存或前端状态。
    #[serde(default)]
    pub configs: BTreeMap<String, Value>,
}

impl PluginSelection {
    pub fn is_enabled(&self, plugin_id: &str) -> bool {
        self.enabled_plugin_ids
            .as_ref()
            .is_none_or(|enabled| enabled.contains(plugin_id))
    }

    pub fn config_for(&self, plugin_id: &str) -> Value {
        self.configs.get(plugin_id).cloned().unwrap_or(Value::Null)
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
    #[serde(default = "default_task_event_capacity_per_task")]
    pub task_event_capacity_per_task: usize,
    #[serde(default = "default_task_event_capacity_total")]
    pub task_event_capacity_total: usize,
    #[serde(default = "default_frontend_event_batch_size")]
    pub frontend_event_batch_size: usize,
    pub preview_ttl_secs: u64,
    #[serde(default)]
    pub plugin_selection: PluginSelection,
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
            task_event_capacity_per_task: default_task_event_capacity_per_task(),
            task_event_capacity_total: default_task_event_capacity_total(),
            frontend_event_batch_size: default_frontend_event_batch_size(),
            preview_ttl_secs: 300,
            plugin_selection: PluginSelection::default(),
            paths: PathsConfig::for_app(&app_name),
            security: SecurityConfig::default(),
            app_name,
        }
    }
}

const fn default_task_event_capacity_per_task() -> usize {
    256
}

const fn default_task_event_capacity_total() -> usize {
    4096
}

const fn default_frontend_event_batch_size() -> usize {
    128
}
