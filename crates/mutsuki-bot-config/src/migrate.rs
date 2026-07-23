//! Schema/value migration helpers — fail loud, never silently drop.

use serde::{Deserialize, Serialize};

use crate::error::{ConfigError, FieldDiff};
use crate::value::ConfigValue;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MigrationPlan {
    pub from_version: u32,
    pub to_version: u32,
    pub steps: Vec<MigrationStep>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MigrationStep {
    RenameField { from: String, to: String },
    DropField { path: String },
    SetDefault { path: String, value: ConfigValue },
}

pub fn migrate(
    value: &ConfigValue,
    plan: &MigrationPlan,
    dry_run: bool,
) -> Result<(ConfigValue, Vec<FieldDiff>), ConfigError> {
    if plan.from_version == plan.to_version {
        return Ok((value.clone(), Vec::new()));
    }
    let mut current = value.clone();
    let mut diff = Vec::new();
    for step in &plan.steps {
        apply_step(&mut current, step, &mut diff)?;
    }
    if dry_run {
        // Caller keeps original; we only return the projected result.
        return Ok((current, diff));
    }
    Ok((current, diff))
}

fn apply_step(
    value: &mut ConfigValue,
    step: &MigrationStep,
    diff: &mut Vec<FieldDiff>,
) -> Result<(), ConfigError> {
    let map = value.as_object_mut().ok_or(ConfigError::ApplyRejected {
        reason: "migration requires object root".into(),
    })?;
    match step {
        MigrationStep::RenameField { from, to } => {
            if let Some(old) = map.remove(from) {
                diff.push(FieldDiff {
                    path: crate::value::ConfigPath(vec![to.clone()]),
                    before: map.get(to).cloned(),
                    after: Some(old.clone()),
                });
                map.insert(to.clone(), old);
            }
        }
        MigrationStep::DropField { path } => {
            if let Some(old) = map.remove(path) {
                diff.push(FieldDiff {
                    path: crate::value::ConfigPath(vec![path.clone()]),
                    before: Some(old),
                    after: None,
                });
            }
        }
        MigrationStep::SetDefault {
            path,
            value: default,
        } => {
            if !map.contains_key(path) {
                map.insert(path.clone(), default.clone());
                diff.push(FieldDiff {
                    path: crate::value::ConfigPath(vec![path.clone()]),
                    before: None,
                    after: Some(default.clone()),
                });
            }
        }
    }
    Ok(())
}

pub fn require_migration(stored_version: u32, current_version: u32) -> Result<(), ConfigError> {
    if stored_version == current_version {
        Ok(())
    } else {
        Err(ConfigError::ValueMigrationRequired {
            from_version: stored_version,
            to_version: current_version,
        })
    }
}
