//! Schema diff engine: compares two
//! [`SchemaCatalog`](crate::introspect::SchemaCatalog)s into a change list
//! plus generated migration SQL.

use serde::{Deserialize, Serialize};

use crate::dump::{schema_sql, DumpOptions};
use crate::introspect::{ObjectType, SchemaCatalog, Table};
use crate::provider::ProviderKind;

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

    /// Concatenate all generated SQL into one migration script (LF endings).
    pub fn sql(&self) -> String {
        self.items
            .iter()
            .filter_map(|item| item.sql.as_deref())
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

/// Compare `source` (desired) against `target` (current).
pub fn diff_catalogs(source: &SchemaCatalog, target: &SchemaCatalog) -> SchemaDiff {
    let kind = source.provider;
    let mut items = Vec::new();

    let src_schemas: Vec<_> = source.schemas.iter().map(|s| s.name.as_str()).collect();
    for name in src_schemas {
        if target.find_schema(name).is_none() {
            items.push(DiffItem {
                kind: ObjectType::Schema,
                action: DiffAction::Create,
                schema: name.into(),
                name: name.into(),
                detail: "schema missing on target".into(),
                sql: Some(format!(
                    "CREATE SCHEMA IF NOT EXISTS {};",
                    kind.quote_ident(name)
                )),
            });
        }
    }

    for src_schema in &source.schemas {
        let tgt_schema = target.find_schema(&src_schema.name);
        for en in &src_schema.enums {
            let exists = tgt_schema
                .map(|s| s.enums.iter().any(|e| e.name == en.name))
                .unwrap_or(false);
            if !exists {
                let labels: Vec<_> = en
                    .labels
                    .iter()
                    .map(|l| crate::sql::quote_string(l))
                    .collect();
                items.push(DiffItem {
                    kind: ObjectType::Enum,
                    action: DiffAction::Create,
                    schema: src_schema.name.clone(),
                    name: en.name.clone(),
                    detail: "enum missing on target".into(),
                    sql: Some(format!(
                        "CREATE TYPE {} AS ENUM ({});",
                        kind.qualify(&src_schema.name, &en.name),
                        labels.join(", ")
                    )),
                });
            }
        }
        for seq in &src_schema.sequences {
            match tgt_schema.and_then(|s| s.sequences.iter().find(|t| t.name == seq.name)) {
                None => items.push(DiffItem {
                    kind: ObjectType::Sequence,
                    action: DiffAction::Create,
                    schema: src_schema.name.clone(),
                    name: seq.name.clone(),
                    detail: "sequence missing on target".into(),
                    sql: Some(format!(
                        "CREATE SEQUENCE {};",
                        kind.qualify(&src_schema.name, &seq.name)
                    )),
                }),
                Some(other) if other.last_value != seq.last_value => items.push(DiffItem {
                    kind: ObjectType::Sequence,
                    action: DiffAction::Alter,
                    schema: src_schema.name.clone(),
                    name: seq.name.clone(),
                    detail: format!(
                        "last_value {} → {:?}",
                        other.last_value.unwrap_or(0),
                        seq.last_value
                    ),
                    sql: seq.last_value.map(|v| {
                        format!(
                            "SELECT setval('{}', {v}, true);",
                            kind.qualify(&src_schema.name, &seq.name).replace('"', "")
                        )
                    }),
                }),
                _ => {}
            }
        }
        for table in &src_schema.tables {
            match tgt_schema.and_then(|s| s.find_table(&table.name)) {
                None => {
                    let mut mini = SchemaCatalog::new(&source.database, kind);
                    let mut sch = crate::introspect::Schema::new(&src_schema.name);
                    sch.tables.push(table.clone());
                    mini.schemas.push(sch);
                    items.push(DiffItem {
                        kind: ObjectType::Table,
                        action: DiffAction::Create,
                        schema: src_schema.name.clone(),
                        name: table.name.clone(),
                        detail: "table missing on target".into(),
                        sql: Some(schema_sql(&mini, &DumpOptions::default())),
                    });
                }
                Some(tgt) => items.extend(diff_table(kind, &src_schema.name, table, tgt)),
            }
        }
        for view in &src_schema.views {
            let exists = tgt_schema
                .map(|s| s.views.iter().any(|v| v.name == view.name))
                .unwrap_or(false);
            if !exists {
                items.push(DiffItem {
                    kind: ObjectType::View,
                    action: DiffAction::Create,
                    schema: src_schema.name.clone(),
                    name: view.name.clone(),
                    detail: "view missing on target".into(),
                    sql: Some(format!(
                        "CREATE VIEW {} AS\n{};",
                        kind.qualify(&src_schema.name, &view.name),
                        view.definition.trim().trim_end_matches(';')
                    )),
                });
            }
        }
        for func in &src_schema.functions {
            let exists = tgt_schema
                .map(|s| s.functions.iter().any(|f| f.name == func.name))
                .unwrap_or(false);
            if !exists {
                items.push(DiffItem {
                    kind: ObjectType::Function,
                    action: DiffAction::Create,
                    schema: src_schema.name.clone(),
                    name: func.name.clone(),
                    detail: "function missing on target".into(),
                    sql: Some(func.definition.clone()),
                });
            }
        }
    }

    if let Some(tgt) = target.schemas.first() {
        if let Some(src) = source.find_schema(&tgt.name) {
            for table in &tgt.tables {
                if src.find_table(&table.name).is_none() {
                    items.push(DiffItem {
                        kind: ObjectType::Table,
                        action: DiffAction::Drop,
                        schema: tgt.name.clone(),
                        name: table.name.clone(),
                        detail: "table only exists on target".into(),
                        sql: Some(format!(
                            "DROP TABLE IF EXISTS {};",
                            kind.qualify(&tgt.name, &table.name)
                        )),
                    });
                }
            }
        }
    }

    SchemaDiff { items }
}

fn diff_table(kind: ProviderKind, schema: &str, src: &Table, tgt: &Table) -> Vec<DiffItem> {
    let mut items = Vec::new();
    for col in &src.columns {
        if !tgt.columns.iter().any(|c| c.name == col.name) {
            items.push(DiffItem {
                kind: ObjectType::Column,
                action: DiffAction::Create,
                schema: schema.into(),
                name: format!("{}.{}", src.name, col.name),
                detail: format!("column {} missing", col.name),
                sql: Some(format!(
                    "ALTER TABLE {} ADD COLUMN {} {}{};",
                    kind.qualify(schema, &src.name),
                    kind.quote_ident(&col.name),
                    col.data_type,
                    if col.nullable { "" } else { " NOT NULL" }
                )),
            });
        }
    }
    for col in &tgt.columns {
        if !src.columns.iter().any(|c| c.name == col.name) {
            items.push(DiffItem {
                kind: ObjectType::Column,
                action: DiffAction::Drop,
                schema: schema.into(),
                name: format!("{}.{}", src.name, col.name),
                detail: format!("column {} only on target", col.name),
                sql: Some(format!(
                    "ALTER TABLE {} DROP COLUMN {};",
                    kind.qualify(schema, &src.name),
                    kind.quote_ident(&col.name)
                )),
            });
        }
    }
    items
}

/// Timestamped migration file name (`YYYYMMDDHHMMSS_from_to.sql`).
pub fn migration_filename(source: &str, target: &str) -> String {
    let ts = chrono::Utc::now().format("%Y%m%d%H%M%S");
    format!("{ts}_{source}_{target}.sql")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::introspect::{Column, Schema, Table};

    fn catalog_with(table: &str) -> SchemaCatalog {
        let mut c = SchemaCatalog::new("db", ProviderKind::Postgres);
        let mut s = Schema::new("public");
        s.tables.push(Table {
            name: table.into(),
            columns: vec![Column {
                name: "id".into(),
                ordinal: 1,
                data_type: "bigint".into(),
                nullable: false,
                default: None,
                is_identity: false,
                identity_generation: None,
                identity_start: None,
                identity_increment: None,
                generated_expr: None,
                comment: None,
            }],
            ..Default::default()
        });
        c.schemas.push(s);
        c
    }

    #[test]
    fn detects_missing_table() {
        let src = catalog_with("users");
        let dst = catalog_with("orders");
        let diff = diff_catalogs(&src, &dst);
        assert!(diff.items.iter().any(|i| i.action == DiffAction::Create
            && i.name == "users"
            && i.kind == ObjectType::Table));
        assert!(diff
            .items
            .iter()
            .any(|i| i.action == DiffAction::Drop && i.name == "orders"));
        assert!(!diff.sql().is_empty());
        assert!(!diff.sql().contains('\r'));
    }
}
