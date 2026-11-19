//! The schema catalog: a complete, provider-agnostic object graph of a
//! database. Everything the transfer, diff and native-dump engines touch is
//! modelled here, so no object type can be silently skipped (the classic
//! "dump lost my sequences" failure mode).

use serde::{Deserialize, Serialize};

use crate::provider::ProviderKind;

/// Kind of database object, used by plans, diffs and verification reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ObjectType {
    /// A schema (Postgres) / database (MySQL).
    Schema,
    /// A table.
    Table,
    /// A view.
    View,
    /// A materialized view.
    MaterializedView,
    /// A sequence.
    Sequence,
    /// A function or stored procedure.
    Function,
    /// A trigger.
    Trigger,
    /// An index.
    Index,
    /// A constraint (PK/FK/CHECK).
    Constraint,
    /// A column.
    Column,
    /// An enum type.
    Enum,
    /// An extension.
    Extension,
}

impl ObjectType {
    /// Canonical lowercase name.
    pub fn as_str(self) -> &'static str {
        match self {
            ObjectType::Schema => "schema",
            ObjectType::Table => "table",
            ObjectType::View => "view",
            ObjectType::MaterializedView => "materialized-view",
            ObjectType::Sequence => "sequence",
            ObjectType::Function => "function",
            ObjectType::Trigger => "trigger",
            ObjectType::Index => "index",
            ObjectType::Constraint => "constraint",
            ObjectType::Column => "column",
            ObjectType::Enum => "enum",
            ObjectType::Extension => "extension",
        }
    }
}

impl std::fmt::Display for ObjectType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A complete snapshot of one database.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SchemaCatalog {
    /// Database name.
    pub database: String,
    /// Engine the catalog was captured from.
    pub provider: ProviderKind,
    /// Capture time, RFC 3333.
    pub captured_at: Option<String>,
    /// All schemas (Postgres) or one pseudo-schema (MySQL/SQLite).
    pub schemas: Vec<Schema>,
}

impl SchemaCatalog {
    /// Empty catalog for a database.
    pub fn new(database: impl Into<String>, provider: ProviderKind) -> Self {
        Self {
            database: database.into(),
            provider,
            captured_at: None,
            schemas: Vec::new(),
        }
    }

    /// Find a schema by name.
    pub fn find_schema(&self, name: &str) -> Option<&Schema> {
        self.schemas.iter().find(|s| s.name == name)
    }

    /// Find a table by schema and name.
    pub fn find_table(&self, schema: &str, table: &str) -> Option<&Table> {
        self.find_schema(schema)?.find_table(table)
    }

    /// Find a sequence by schema and name.
    pub fn find_sequence(&self, schema: &str, sequence: &str) -> Option<&Sequence> {
        self.find_schema(schema)?
            .sequences
            .iter()
            .find(|s| s.name == sequence)
    }

    /// Count objects by type — the basis of post-transfer verification.
    pub fn object_counts(&self) -> ObjectCounts {
        let mut counts = ObjectCounts {
            schemas: self.schemas.len(),
            ..Default::default()
        };
        for schema in &self.schemas {
            counts.tables += schema.tables.len();
            counts.views += schema.views.len();
            counts.materialized_views += schema.materialized_views.len();
            counts.sequences += schema.sequences.len();
            counts.functions += schema.functions.len();
            counts.enums += schema.enums.len();
            counts.extensions += schema.extensions.len();
            for table in &schema.tables {
                counts.columns += table.columns.len();
                counts.indexes += table.indexes.len();
                counts.triggers += table.triggers.len();
                counts.constraints += table.foreign_keys.len()
                    + table.checks.len()
                    + usize::from(table.primary_key.is_some());
            }
        }
        counts
    }

    /// Total estimated rows across all tables.
    pub fn estimated_rows(&self) -> u64 {
        self.schemas
            .iter()
            .flat_map(|s| s.tables.iter())
            .map(|t| t.row_estimate.unwrap_or(0))
            .sum()
    }
}

/// Per-type object counts, compared between source and target during
/// verification.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectCounts {
    /// Number of schemas.
    pub schemas: usize,
    /// Number of tables.
    pub tables: usize,
    /// Number of columns.
    pub columns: usize,
    /// Number of views.
    pub views: usize,
    /// Number of materialized views.
    pub materialized_views: usize,
    /// Number of sequences.
    pub sequences: usize,
    /// Number of functions/procedures.
    pub functions: usize,
    /// Number of triggers.
    pub triggers: usize,
    /// Number of indexes.
    pub indexes: usize,
    /// Number of constraints (PK + FK + CHECK).
    pub constraints: usize,
    /// Number of enum types.
    pub enums: usize,
    /// Number of extensions.
    pub extensions: usize,
}

/// A Postgres schema, or a single pseudo-schema for engines without them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Schema {
    /// Schema name (`public` in Postgres; database name in MySQL).
    pub name: String,
    /// Tables.
    pub tables: Vec<Table>,
    /// Regular views.
    pub views: Vec<View>,
    /// Materialized views.
    pub materialized_views: Vec<View>,
    /// Sequences (including those owned by identity/serial columns).
    pub sequences: Vec<Sequence>,
    /// Functions and procedures.
    pub functions: Vec<Function>,
    /// Enum types.
    pub enums: Vec<EnumType>,
    /// Installed extensions.
    pub extensions: Vec<Extension>,
}

impl Schema {
    /// Create an empty schema.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    /// Find a table by name.
    pub fn find_table(&self, name: &str) -> Option<&Table> {
        self.tables.iter().find(|t| t.name == name)
    }
}

/// A table with its full dependency surface.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Table {
    /// Table name.
    pub name: String,
    /// Comment, if any.
    pub comment: Option<String>,
    /// Columns in ordinal order.
    pub columns: Vec<Column>,
    /// Primary key.
    pub primary_key: Option<PrimaryKey>,
    /// Foreign keys (needed for dependency-ordered DDL).
    pub foreign_keys: Vec<ForeignKey>,
    /// CHECK constraints.
    pub checks: Vec<CheckConstraint>,
    /// Indexes (including unique indexes backing constraints).
    pub indexes: Vec<Index>,
    /// Triggers attached to this table.
    pub triggers: Vec<Trigger>,
    /// Estimated row count from planner statistics (None = unknown).
    pub row_estimate: Option<u64>,
}

/// A column definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Column {
    /// Column name.
    pub name: String,
    /// 1-based ordinal position.
    pub ordinal: u32,
    /// Raw type as reported by the server (`bigint`, `varchar(255)`, ...).
    pub data_type: String,
    /// Whether NULL is allowed.
    pub nullable: bool,
    /// Default expression, verbatim.
    pub default: Option<String>,
    /// True for identity / serial / auto-increment columns.
    pub is_identity: bool,
    /// `ALWAYS` or `BY DEFAULT` for SQL identity columns.
    pub identity_generation: Option<String>,
    /// Identity seed.
    pub identity_start: Option<i64>,
    /// Identity increment.
    pub identity_increment: Option<i64>,
    /// Expression for generated (computed) columns.
    pub generated_expr: Option<String>,
    /// Column comment.
    pub comment: Option<String>,
}

/// A primary key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrimaryKey {
    /// Constraint name, if named.
    pub name: Option<String>,
    /// Key columns in order.
    pub columns: Vec<String>,
}

/// A foreign key referencing another table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForeignKey {
    /// Constraint name.
    pub name: String,
    /// Local columns.
    pub columns: Vec<String>,
    /// Referenced schema.
    pub ref_schema: String,
    /// Referenced table.
    pub ref_table: String,
    /// Referenced columns.
    pub ref_columns: Vec<String>,
    /// ON DELETE action (`CASCADE`, `SET NULL`, ...).
    pub on_delete: Option<String>,
    /// ON UPDATE action.
    pub on_update: Option<String>,
}

/// A CHECK constraint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckConstraint {
    /// Constraint name.
    pub name: String,
    /// Boolean expression, verbatim.
    pub expression: String,
}

/// An index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Index {
    /// Index name.
    pub name: String,
    /// Column names, or expressions for expression indexes.
    pub columns: Vec<String>,
    /// Whether the index enforces uniqueness.
    pub unique: bool,
    /// Access method (`btree`, `gin`, `hash`, ...).
    pub method: Option<String>,
    /// WHERE clause for partial indexes.
    pub partial: Option<String>,
}

/// A sequence object, including its current value — the piece dumps commonly
/// forget, leaving fresh IDs colliding with restored rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sequence {
    /// Sequence name.
    pub name: String,
    /// Data type (`smallint`, `integer`, `bigint`).
    pub data_type: String,
    /// Seed value.
    pub start: i64,
    /// Increment.
    pub increment: i64,
    /// Minimum value.
    pub min: i128,
    /// Maximum value.
    pub max: i128,
    /// Cache size.
    pub cache: i64,
    /// Whether the sequence wraps around.
    pub cycle: bool,
    /// Current value (`last_value`), None if never called.
    pub last_value: Option<i128>,
    /// Owning `(table, column)` for serial/identity-owned sequences.
    pub owned_by: Option<(String, String)>,
}

/// A view or materialized view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct View {
    /// View name.
    pub name: String,
    /// SELECT definition, verbatim.
    pub definition: String,
    /// Whether this is a materialized view.
    pub materialized: bool,
    /// Output column names.
    pub columns: Vec<String>,
}

/// A function or stored procedure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Function {
    /// Function name.
    pub name: String,
    /// Function vs procedure.
    pub kind: FunctionKind,
    /// Implementation language (`sql`, `plpgsql`, ...).
    pub language: String,
    /// Argument signatures as `name type` strings.
    pub args: Vec<String>,
    /// Return type (`SETOF record`, `void`, ...).
    pub return_type: String,
    /// Body / definition, verbatim.
    pub definition: String,
    /// Whether the function is strict (returns NULL on NULL input).
    pub is_strict: bool,
    /// Volatility category (`immutable`, `stable`, `volatile`).
    pub volatility: Option<String>,
}

/// Function vs procedure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FunctionKind {
    /// A function.
    Function,
    /// A stored procedure.
    Procedure,
}

/// A trigger.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trigger {
    /// Trigger name.
    pub name: String,
    /// `BEFORE`, `AFTER` or `INSTEAD OF`.
    pub timing: String,
    /// Events: any of `INSERT`, `UPDATE`, `DELETE`.
    pub events: Vec<String>,
    /// `ROW` or `STATEMENT`.
    pub orientation: String,
    /// WHEN condition, if any.
    pub condition: Option<String>,
    /// Trigger body / function call.
    pub definition: String,
}

/// An enum type (Postgres) — must be created before tables that use it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnumType {
    /// Type name.
    pub name: String,
    /// Labels in declaration order.
    pub labels: Vec<String>,
}

/// An installed extension (Postgres).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Extension {
    /// Extension name.
    pub name: String,
    /// Installed version.
    pub version: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_catalog() -> SchemaCatalog {
        let mut catalog = SchemaCatalog::new("appdb", ProviderKind::Postgres);
        let mut schema = Schema::new("public");
        schema.sequences.push(Sequence {
            name: "users_id_seq".into(),
            data_type: "bigint".into(),
            start: 1,
            increment: 1,
            min: 1,
            max: i64::MAX as i128,
            cache: 1,
            cycle: false,
            last_value: Some(42),
            owned_by: Some(("users".into(), "id".into())),
        });
        schema.tables.push(Table {
            name: "users".into(),
            columns: vec![Column {
                name: "id".into(),
                ordinal: 1,
                data_type: "bigint".into(),
                nullable: false,
                default: Some("nextval('users_id_seq')".into()),
                is_identity: false,
                identity_generation: None,
                identity_start: None,
                identity_increment: None,
                generated_expr: None,
                comment: None,
            }],
            primary_key: Some(PrimaryKey {
                name: Some("users_pkey".into()),
                columns: vec!["id".into()],
            }),
            row_estimate: Some(42),
            ..Default::default()
        });
        catalog.schemas.push(schema);
        catalog
    }

    #[test]
    fn find_table_and_sequence() {
        let catalog = sample_catalog();
        assert!(catalog.find_table("public", "users").is_some());
        assert!(catalog.find_table("public", "orders").is_none());
        let seq = catalog.find_sequence("public", "users_id_seq").unwrap();
        assert_eq!(seq.last_value, Some(42));
        assert_eq!(seq.owned_by.as_ref().unwrap().0, "users");
    }

    #[test]
    fn object_counts_feed_verification() {
        let counts = sample_catalog().object_counts();
        assert_eq!(counts.tables, 1);
        assert_eq!(counts.columns, 1);
        assert_eq!(counts.sequences, 1);
        assert_eq!(counts.constraints, 1); // just the PK
    }

    #[test]
    fn estimated_rows_sums_tables() {
        assert_eq!(sample_catalog().estimated_rows(), 42);
    }
}
