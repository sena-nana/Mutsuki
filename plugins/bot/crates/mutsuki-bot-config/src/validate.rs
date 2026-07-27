//! Structural validation against ConfigDescriptor.

use crate::budgets::{ConfigBudgets, DEFAULT_BUDGETS};
use crate::error::{
    LocalizedText, ValidationCode, ValidationIssue, ValidationResult, ValidationSeverity,
};
use crate::schema::{ConfigDescriptor, ConfigNode, ConfigValueType};
use crate::secret::SecretState;
use crate::value::{ConfigPath, ConfigValue};

pub fn validate_structure(
    descriptor: &ConfigDescriptor,
    candidate: &ConfigValue,
) -> ValidationResult {
    validate_structure_with_budgets(descriptor, candidate, &DEFAULT_BUDGETS)
}

pub fn validate_structure_with_budgets(
    descriptor: &ConfigDescriptor,
    candidate: &ConfigValue,
    budgets: &ConfigBudgets,
) -> ValidationResult {
    let mut issues = Vec::new();
    validate_node(
        &descriptor.root,
        candidate,
        &ConfigPath::root(),
        budgets,
        &mut issues,
    );
    if issues.len() > budgets.max_validation_issues {
        issues.truncate(budgets.max_validation_issues);
        issues.push(ValidationIssue {
            path: ConfigPath::root(),
            code: ValidationCode::BudgetExceeded,
            severity: ValidationSeverity::Error,
            message: LocalizedText::new("too many validation issues"),
        });
    }
    ValidationResult::from_issues(issues)
}

fn validate_node(
    node: &ConfigNode,
    value: &ConfigValue,
    path: &ConfigPath,
    budgets: &ConfigBudgets,
    issues: &mut Vec<ValidationIssue>,
) {
    if let Some(expr) = &node.visibility {
        let root = value_root_for_visibility(value, path);
        if let Ok(false) = expr.eval(root) {
            return;
        }
    }

    match &node.value_type {
        ConfigValueType::Object => {
            let Some(map) = value.as_object() else {
                push(
                    issues,
                    path,
                    ValidationCode::TypeMismatch,
                    "expected object",
                );
                return;
            };
            let known: std::collections::BTreeSet<_> = node
                .children
                .iter()
                .map(|c| c.key.as_str().to_string())
                .collect();
            for key in map.keys() {
                if !known.contains(key) {
                    push(
                        issues,
                        &path.join(key.clone()),
                        ValidationCode::UnknownField,
                        format!("unknown field `{key}`"),
                    );
                }
            }
            for child in &node.children {
                let child_path = path.join(child.key.as_str());
                match map.get(child.key.as_str()) {
                    Some(child_value) => {
                        validate_leaf_or_nested(child, child_value, &child_path, budgets, issues)
                    }
                    None if child.constraints.required => push(
                        issues,
                        &child_path,
                        ValidationCode::Required,
                        format!("missing required field `{}`", child.key.as_str()),
                    ),
                    None => {}
                }
            }
        }
        other => validate_typed(node, other, value, path, budgets, issues),
    }
}

fn validate_leaf_or_nested(
    node: &ConfigNode,
    value: &ConfigValue,
    path: &ConfigPath,
    budgets: &ConfigBudgets,
    issues: &mut Vec<ValidationIssue>,
) {
    if matches!(node.value_type, ConfigValueType::Object) || !node.children.is_empty() {
        validate_node(node, value, path, budgets, issues);
    } else {
        validate_typed(node, &node.value_type, value, path, budgets, issues);
    }
}

fn validate_typed(
    node: &ConfigNode,
    ty: &ConfigValueType,
    value: &ConfigValue,
    path: &ConfigPath,
    budgets: &ConfigBudgets,
    issues: &mut Vec<ValidationIssue>,
) {
    match (ty, value) {
        (ConfigValueType::Null, ConfigValue::Null) => {}
        (ConfigValueType::Bool, ConfigValue::Bool(_)) => {}
        (ConfigValueType::Integer, ConfigValue::Integer(v)) => {
            check_range(node, *v as f64, path, issues)
        }
        (ConfigValueType::Float, ConfigValue::Float(v)) => check_range(node, *v, path, issues),
        (ConfigValueType::Float, ConfigValue::Integer(v)) => {
            check_range(node, *v as f64, path, issues)
        }
        (ConfigValueType::String { .. }, ConfigValue::String(s)) => {
            if s.len() > budgets.max_string_bytes {
                push(
                    issues,
                    path,
                    ValidationCode::BudgetExceeded,
                    "string exceeds budget",
                );
            }
            check_length(node, s.len(), path, issues);
            if let Some(pattern) = &node.constraints.pattern {
                if regex_is_match(pattern, s).is_err() {
                    push(
                        issues,
                        path,
                        ValidationCode::PatternMismatch,
                        "value does not match pattern",
                    );
                }
            }
        }
        (ConfigValueType::Secret, ConfigValue::Secret(state)) => {
            if matches!(state, SecretState::Set { value } if value.expose().len() > budgets.max_string_bytes)
            {
                push(
                    issues,
                    path,
                    ValidationCode::BudgetExceeded,
                    "secret exceeds budget",
                );
            }
        }
        (ConfigValueType::Enum { options, multi }, ConfigValue::String(s)) if !multi => {
            if !options.iter().any(|opt| opt.value == *s) {
                push(
                    issues,
                    path,
                    ValidationCode::ConstraintFailed,
                    "invalid enum",
                );
            }
        }
        (ConfigValueType::Enum { options, multi }, ConfigValue::Array(items)) if *multi => {
            if items.len() > budgets.max_array_len {
                push(
                    issues,
                    path,
                    ValidationCode::BudgetExceeded,
                    "array exceeds budget",
                );
            }
            for item in items {
                match item {
                    ConfigValue::String(s) if options.iter().any(|opt| opt.value == *s) => {}
                    _ => push(
                        issues,
                        path,
                        ValidationCode::ConstraintFailed,
                        "invalid multi-select value",
                    ),
                }
            }
        }
        (ConfigValueType::Array { item }, ConfigValue::Array(items)) => {
            if items.len() > budgets.max_array_len {
                push(
                    issues,
                    path,
                    ValidationCode::BudgetExceeded,
                    "array exceeds budget",
                );
            }
            check_items(node, items.len(), path, issues);
            for (idx, entry) in items.iter().enumerate() {
                let child_path = path.join(idx.to_string());
                validate_typed(node, item, entry, &child_path, budgets, issues);
            }
        }
        (ConfigValueType::Map { .. }, ConfigValue::Object(map)) => {
            if map.len() > budgets.max_map_entries {
                push(
                    issues,
                    path,
                    ValidationCode::BudgetExceeded,
                    "map exceeds budget",
                );
            }
        }
        (ConfigValueType::FileRef | ConfigValueType::DirectoryRef, ConfigValue::String(_)) => {}
        _ => push(issues, path, ValidationCode::TypeMismatch, "type mismatch"),
    }
}

fn check_range(
    node: &ConfigNode,
    value: f64,
    path: &ConfigPath,
    issues: &mut Vec<ValidationIssue>,
) {
    if let Some(min) = node.constraints.min {
        if value < min {
            push(issues, path, ValidationCode::OutOfRange, "below minimum");
        }
    }
    if let Some(max) = node.constraints.max {
        if value > max {
            push(issues, path, ValidationCode::OutOfRange, "above maximum");
        }
    }
}

fn check_length(
    node: &ConfigNode,
    len: usize,
    path: &ConfigPath,
    issues: &mut Vec<ValidationIssue>,
) {
    if let Some(min) = node.constraints.min_length {
        if len < min {
            push(issues, path, ValidationCode::LengthInvalid, "too short");
        }
    }
    if let Some(max) = node.constraints.max_length {
        if len > max {
            push(issues, path, ValidationCode::LengthInvalid, "too long");
        }
    }
}

fn check_items(
    node: &ConfigNode,
    len: usize,
    path: &ConfigPath,
    issues: &mut Vec<ValidationIssue>,
) {
    if let Some(min) = node.constraints.min_items {
        if len < min {
            push(issues, path, ValidationCode::LengthInvalid, "too few items");
        }
    }
    if let Some(max) = node.constraints.max_items {
        if len > max {
            push(
                issues,
                path,
                ValidationCode::LengthInvalid,
                "too many items",
            );
        }
    }
}

fn push(
    issues: &mut Vec<ValidationIssue>,
    path: &ConfigPath,
    code: ValidationCode,
    message: impl Into<String>,
) {
    issues.push(ValidationIssue {
        path: path.clone(),
        code,
        severity: ValidationSeverity::Error,
        message: LocalizedText::new(message),
    });
}

fn value_root_for_visibility<'a>(value: &'a ConfigValue, _path: &ConfigPath) -> &'a ConfigValue {
    // Visibility expressions reference sibling fields at the object root being edited.
    value
}

fn regex_is_match(pattern: &str, value: &str) -> Result<(), ()> {
    // Lightweight subset: avoid pulling regex crate into protocol core for MVP.
    // Support ^...$ exact and contains-style patterns via simple checks.
    if pattern.starts_with('^') && pattern.ends_with('$') {
        let inner = &pattern[1..pattern.len() - 1];
        if inner
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./".contains(c))
        {
            return if value == inner { Ok(()) } else { Err(()) };
        }
    }
    if value.contains(pattern) || pattern.is_empty() {
        Ok(())
    } else {
        // Treat unsupported patterns as "must equal" for safety.
        if value == pattern { Ok(()) } else { Err(()) }
    }
}
