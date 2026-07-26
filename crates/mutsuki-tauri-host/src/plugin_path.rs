//! 插件包内部路径的跨平台安全规范化。

use std::path::{Component, Path, PathBuf};

pub(crate) fn safe_relative_path(value: &str) -> Result<PathBuf, String> {
    if value.is_empty()
        || value.starts_with('/')
        || value.starts_with('\\')
        || value.contains('\\')
        || value.as_bytes().contains(&0)
    {
        return Err(format!("unsafe plugin archive path: {value}"));
    }
    let path = Path::new(value);
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(value) if value != "." && value != ".." => normalized.push(value),
            _ => return Err(format!("unsafe plugin archive path: {value}")),
        }
    }
    if normalized.as_os_str().is_empty() || normalized.is_absolute() {
        return Err(format!("unsafe plugin archive path: {value}"));
    }
    Ok(normalized)
}

pub(crate) fn component_text(component: Component<'_>) -> Option<&str> {
    match component {
        Component::Normal(value) => value.to_str(),
        _ => None,
    }
}

pub(crate) fn relative_path_string(path: &Path) -> String {
    path.components()
        .filter_map(component_text)
        .collect::<Vec<_>>()
        .join("/")
}
