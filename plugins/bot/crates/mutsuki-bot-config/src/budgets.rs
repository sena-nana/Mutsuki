//! Hard resource budgets for schema/provider safety.

/// Absolute limits for schema size, expression complexity, and values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigBudgets {
    pub max_providers: usize,
    pub max_schema_nodes: usize,
    pub max_schema_depth: usize,
    pub max_array_len: usize,
    pub max_map_entries: usize,
    pub max_string_bytes: usize,
    pub max_expr_nodes: usize,
    pub max_expr_depth: usize,
    pub max_validation_issues: usize,
    pub max_id_bytes: usize,
}

impl Default for ConfigBudgets {
    fn default() -> Self {
        Self {
            max_providers: 256,
            max_schema_nodes: 2_048,
            max_schema_depth: 16,
            max_array_len: 256,
            max_map_entries: 256,
            max_string_bytes: 16 * 1024,
            max_expr_nodes: 128,
            max_expr_depth: 12,
            max_validation_issues: 64,
            max_id_bytes: 128,
        }
    }
}

/// Shared default budgets.
pub const DEFAULT_BUDGETS: ConfigBudgets = ConfigBudgets {
    max_providers: 256,
    max_schema_nodes: 2_048,
    max_schema_depth: 16,
    max_array_len: 256,
    max_map_entries: 256,
    max_string_bytes: 16 * 1024,
    max_expr_nodes: 128,
    max_expr_depth: 12,
    max_validation_issues: 64,
    max_id_bytes: 128,
};
