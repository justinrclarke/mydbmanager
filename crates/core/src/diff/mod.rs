//! Schema diff engine (Phase 3): compares two
//! [`SchemaCatalog`](crate::introspect::SchemaCatalog)s into a change list
//! plus generated migration SQL.

use serde::{Deserialize, Serialize};

use crate::introspect::ObjectType;

/// What must happen to an object to bring target in line with source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffAction {
    /// Object exists in source but not target.
    Create,
    /// Object exists in both but differs.
    Alter,
    /// Object exists in target but not source.
    Drop,
}

/// A single difference between two catalogs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffItem {
    /// Kind of object.
    pub kind: ObjectType,
    /// Required action.
    pub action: DiffAction,
    /// Schema name.
    pub schema: String,
    /// Object name.
    pub name: String,
    /// Human-readable explanation of the difference.
    pub detail: String,
    /// Generated SQL, when the engine can produce it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sql: Option<String>,
}

/// The full diff between a source and target database.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SchemaDiff {
    /// All differences, in dependency-safe application order.
    pub items: Vec<DiffItem>,
}

impl SchemaDiff {
    /// True when source and target match.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Concatenate all generated SQL into one migration script.
    pub fn sql(&self) -> String {
        self.items
            .iter()
            .filter_map(|item| item.sql.as_deref())
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}
