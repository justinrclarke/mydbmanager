//! MySQL/MariaDB provider: connection, catalog introspection via
//! `information_schema`, and query execution via `sqlx`.

use std::collections::BTreeMap;
use std::time::Instant;

use async_trait::async_trait;
use sqlx::mysql::{MySqlPool, MySqlPoolOptions, MySqlRow};
use sqlx::{Column as _, Row as _, TypeInfo as _, ValueRef as _};

use crate::conn::ConnectionProfile;
use crate::error::{MyDbError, Result};
use crate::introspect::*;
use crate::provider::{DatabaseProvider, ProviderKind};
use crate::providers::{
    build_url, column_meta, decode_error, is_row_returning, timed, unsupported, ACQUIRE_TIMEOUT,
    MAX_CONNECTIONS,
};
use crate::query::{QueryResult, Value};

/// Connected (or connectable) MySQL/MariaDB database.
pub struct MysqlProvider {
    profile: ConnectionProfile,
    pool: Option<MySqlPool>,
}

impl MysqlProvider {
    /// Create an unconnected provider for a profile.
    pub fn new(profile: ConnectionProfile) -> Self {
        Self {
            profile,
            pool: None,
        }
    }

    fn pool(&self) -> Result<&MySqlPool> {
        self.pool
            .as_ref()
            .ok_or_else(|| MyDbError::Database("not connected".into()))
    }
}

#[async_trait]
impl DatabaseProvider for MysqlProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Mysql
    }

    async fn connect(&mut self) -> Result<()> {
        let url = build_url(&self.profile)?;
        let pool = MySqlPoolOptions::new()
            .max_connections(MAX_CONNECTIONS)
            .acquire_timeout(ACQUIRE_TIMEOUT)
            .connect(&url)
            .await?;
        self.pool = Some(pool);
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        if let Some(pool) = self.pool.take() {
            pool.close().await;
        }
        Ok(())
    }

    async fn introspect(&self) -> Result<SchemaCatalog> {
        let pool = self.pool()?;
        let database: String = sqlx::query_scalar("SELECT DATABASE()")
            .fetch_one(pool)
            .await?;
        let mut catalog = SchemaCatalog::new(&database, ProviderKind::Mysql);
        catalog.captured_at = Some(chrono::Utc::now().to_rfc3339());

        // MySQL: one "schema" == the connected database.
        let mut schema = Schema::new(&database);
        schema.tables = introspect_tables(pool, &database).await?;
        attach_primary_keys(pool, &database, &mut schema.tables).await?;
        attach_foreign_keys(pool, &database, &mut schema.tables).await?;
        attach_checks(pool, &database, &mut schema.tables).await?;
        attach_indexes(pool, &database, &mut schema.tables).await?;
        attach_triggers(pool, &database, &mut schema.tables).await?;
        schema.views = introspect_views(pool, &database).await?;
        schema.functions = introspect_routines(pool, &database).await?;
        catalog.schemas.push(schema);
        Ok(catalog)
    }

    async fn execute_query(&self, sql: &str) -> Result<QueryResult> {
        let pool = self.pool()?;
        let start = Instant::now();
        if !is_row_returning(sql) {
            let done = sqlx::query(sql).execute(pool).await?;
            return Ok(QueryResult {
                rows_affected: done.rows_affected(),
                duration: timed(start),
                ..Default::default()
            });
        }
        let rows = sqlx::query(sql).fetch_all(pool).await?;
        let columns = rows
            .first()
            .map(|r| {
                r.columns()
                    .iter()
                    .map(|c| column_meta(c.name(), c.type_info().name()))
                    .collect()
            })
            .unwrap_or_default();
        let mut result = QueryResult {
            columns,
            rows: Vec::with_capacity(rows.len()),
            rows_affected: rows.len() as u64,
            duration: std::time::Duration::ZERO,
        };
        for row in &rows {
            let values = (0..row.columns().len())
                .map(|i| mysql_value(row, i))
                .collect::<Result<Vec<_>>>()?;
            result.rows.push(values);
        }
        result.duration = timed(start);
        Ok(result)
    }

    async fn server_version(&self) -> Result<String> {
        let pool = self.pool()?;
        let version: String = sqlx::query_scalar("SELECT VERSION()")
            .fetch_one(pool)
            .await?;
        Ok(format!("MySQL {version}"))
    }
}

async fn introspect_tables(pool: &MySqlPool, db: &str) -> Result<Vec<Table>> {
    let rows = sqlx::query(
        "SELECT table_name AS name, table_comment AS comment, table_rows AS row_estimate
         FROM information_schema.tables
         WHERE table_schema = ? AND table_type = 'BASE TABLE'
         ORDER BY table_name",
    )
    .bind(db)
    .fetch_all(pool)
    .await?;

    let mut tables = Vec::with_capacity(rows.len());
    for row in rows {
        let name: String = row.try_get("name")?;
        let comment: Option<String> = row.try_get("comment")?;
        tables.push(Table {
            name: name.clone(),
            comment: comment.filter(|c| !c.is_empty()),
            row_estimate: row
                .try_get::<Option<i64>, _>("row_estimate")?
                .map(|v| v.max(0) as u64),
            columns: introspect_columns(pool, db, &name).await?,
            ..Default::default()
        });
    }
    Ok(tables)
}

async fn introspect_columns(pool: &MySqlPool, db: &str, table: &str) -> Result<Vec<Column>> {
    let rows = sqlx::query(
        "SELECT column_name AS name,
                ordinal_position AS ordinal,
                column_type AS data_type,
                is_nullable = 'YES' AS nullable,
                column_default AS default_expr,
                extra,
                generation_expression,
                column_comment AS comment
         FROM information_schema.columns
         WHERE table_schema = ? AND table_name = ?
         ORDER BY ordinal_position",
    )
    .bind(db)
    .bind(table)
    .fetch_all(pool)
    .await?;

    let mut columns = Vec::with_capacity(rows.len());
    for row in rows {
        let extra: String = row.try_get("extra")?;
        let generated: Option<String> = row.try_get("generation_expression")?;
        let is_identity = extra.contains("auto_increment");
        columns.push(Column {
            name: row.try_get("name")?,
            ordinal: row.try_get::<u32, _>("ordinal")?,
            data_type: row.try_get("data_type")?,
            nullable: row.try_get("nullable")?,
            default: row.try_get("default_expr")?,
            is_identity,
            identity_generation: is_identity.then(|| "BY DEFAULT".to_string()),
            identity_start: None,
            identity_increment: None,
            generated_expr: generated.filter(|g| !g.is_empty()),
            comment: row
                .try_get::<Option<String>, _>("comment")?
                .filter(|c| !c.is_empty()),
        });
    }
    Ok(columns)
}

async fn attach_primary_keys(pool: &MySqlPool, db: &str, tables: &mut [Table]) -> Result<()> {
    let rows = sqlx::query(
        "SELECT table_name, column_name, ordinal_position
         FROM information_schema.key_column_usage
         WHERE table_schema = ? AND constraint_name = 'PRIMARY'
         ORDER BY table_name, ordinal_position",
    )
    .bind(db)
    .fetch_all(pool)
    .await?;

    let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in rows {
        let table: String = row.try_get("table_name")?;
        let column: String = row.try_get("column_name")?;
        grouped.entry(table).or_default().push(column);
    }
    for table in tables.iter_mut() {
        if let Some(columns) = grouped.remove(&table.name) {
            table.primary_key = Some(PrimaryKey {
                name: Some("PRIMARY".into()),
                columns,
            });
        }
    }
    Ok(())
}

async fn attach_foreign_keys(pool: &MySqlPool, db: &str, tables: &mut [Table]) -> Result<()> {
    let rows = sqlx::query(
        "SELECT kcu.table_name,
                kcu.constraint_name AS name,
                kcu.column_name,
                kcu.ordinal_position,
                kcu.referenced_table_schema AS ref_schema,
                kcu.referenced_table_name AS ref_table,
                rc.delete_rule,
                rc.update_rule
         FROM information_schema.key_column_usage kcu
         JOIN information_schema.referential_constraints rc
           ON rc.constraint_schema = kcu.table_schema
          AND rc.constraint_name = kcu.constraint_name
          AND rc.table_name = kcu.table_name
         WHERE kcu.table_schema = ? AND kcu.referenced_table_name IS NOT NULL
         ORDER BY kcu.table_name, kcu.constraint_name, kcu.ordinal_position",
    )
    .bind(db)
    .fetch_all(pool)
    .await?;

    // Group columns per (table, constraint); referenced columns resolve by position.
    #[derive(Default)]
    struct FkParts {
        ref_schema: String,
        ref_table: String,
        on_delete: Option<String>,
        on_update: Option<String>,
        columns: Vec<String>,
    }
    let mut grouped: BTreeMap<(String, String), FkParts> = BTreeMap::new();
    for row in rows {
        let table: String = row.try_get("table_name")?;
        let name: String = row.try_get("name")?;
        let entry = grouped.entry((table, name)).or_default();
        entry.columns.push(row.try_get("column_name")?);
        entry.ref_schema = row.try_get("ref_schema")?;
        entry.ref_table = row.try_get("ref_table")?;
        entry.on_delete = row.try_get("delete_rule")?;
        entry.on_update = row.try_get("update_rule")?;
    }

    for ((table_name, name), parts) in grouped {
        // Resolve referenced column names from the referenced table's PK order.
        let ref_columns: Vec<String> = sqlx::query_scalar(
            "SELECT column_name
             FROM information_schema.key_column_usage
             WHERE table_schema = ? AND table_name = ? AND constraint_name = 'PRIMARY'
             ORDER BY ordinal_position",
        )
        .bind(&parts.ref_schema)
        .bind(&parts.ref_table)
        .fetch_all(pool)
        .await?;
        if let Some(t) = tables.iter_mut().find(|t| t.name == table_name) {
            t.foreign_keys.push(ForeignKey {
                name,
                columns: parts.columns,
                ref_schema: parts.ref_schema,
                ref_table: parts.ref_table,
                ref_columns,
                on_delete: parts.on_delete,
                on_update: parts.on_update,
            });
        }
    }
    Ok(())
}

async fn attach_checks(pool: &MySqlPool, db: &str, tables: &mut [Table]) -> Result<()> {
    // MySQL 8.0.16+ / MariaDB 10.2+; ignore engines without the table.
    let rows = match sqlx::query(
        "SELECT tc.table_name, cc.constraint_name AS name, cc.check_clause AS expression
         FROM information_schema.check_constraints cc
         JOIN information_schema.table_constraints tc
           ON tc.constraint_schema = cc.constraint_schema
          AND tc.constraint_name = cc.constraint_name
         WHERE cc.constraint_schema = ? AND tc.constraint_type = 'CHECK'
         ORDER BY cc.constraint_name",
    )
    .bind(db)
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(sqlx::Error::Database(e)) if e.to_string().contains("1109") => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    for row in rows {
        let table: String = row.try_get("table_name")?;
        if let Some(t) = tables.iter_mut().find(|t| t.name == table) {
            t.checks.push(CheckConstraint {
                name: row.try_get("name")?,
                expression: row.try_get("expression")?,
            });
        }
    }
    Ok(())
}

async fn attach_indexes(pool: &MySqlPool, db: &str, tables: &mut [Table]) -> Result<()> {
    let rows = sqlx::query(
        "SELECT table_name, index_name, non_unique, index_type, seq_in_index, column_name
         FROM information_schema.statistics
         WHERE table_schema = ?
         ORDER BY table_name, index_name, seq_in_index",
    )
    .bind(db)
    .fetch_all(pool)
    .await?;

    #[derive(Default)]
    struct IdxParts {
        unique: bool,
        method: Option<String>,
        columns: Vec<String>,
    }
    let mut grouped: BTreeMap<(String, String), IdxParts> = BTreeMap::new();
    for row in rows {
        let table: String = row.try_get("table_name")?;
        let index: String = row.try_get("index_name")?;
        let non_unique: i64 = row.try_get("non_unique")?;
        let method: Option<String> = row.try_get("index_type")?;
        let column: Option<String> = row.try_get("column_name")?;
        let entry = grouped.entry((table, index)).or_default();
        entry.unique = non_unique == 0;
        entry.method = method;
        entry
            .columns
            .push(column.unwrap_or_else(|| "<expr>".into()));
    }

    for ((table_name, name), parts) in grouped {
        if name == "PRIMARY" {
            continue; // already modelled as the primary key
        }
        if let Some(t) = tables.iter_mut().find(|t| t.name == table_name) {
            t.indexes.push(Index {
                name,
                columns: parts.columns,
                unique: parts.unique,
                method: parts.method,
                partial: None,
            });
        }
    }
    Ok(())
}

async fn attach_triggers(pool: &MySqlPool, db: &str, tables: &mut [Table]) -> Result<()> {
    let rows = sqlx::query(
        "SELECT trigger_name AS name, event_object_table AS table_name,
                action_timing AS timing, event_manipulation AS event,
                action_orientation AS orientation, action_statement AS definition
         FROM information_schema.triggers
         WHERE trigger_schema = ?
         ORDER BY trigger_name",
    )
    .bind(db)
    .fetch_all(pool)
    .await?;

    for row in rows {
        let table: String = row.try_get("table_name")?;
        if let Some(t) = tables.iter_mut().find(|t| t.name == table) {
            t.triggers.push(Trigger {
                name: row.try_get("name")?,
                timing: row.try_get("timing")?,
                events: vec![row.try_get("event")?],
                orientation: row.try_get("orientation")?,
                condition: None,
                definition: row.try_get("definition")?,
            });
        }
    }
    Ok(())
}

async fn introspect_views(pool: &MySqlPool, db: &str) -> Result<Vec<View>> {
    let rows = sqlx::query(
        "SELECT table_name AS name, view_definition AS definition
         FROM information_schema.views
         WHERE table_schema = ?
         ORDER BY table_name",
    )
    .bind(db)
    .fetch_all(pool)
    .await?;

    let mut views = Vec::with_capacity(rows.len());
    for row in rows {
        views.push(View {
            name: row.try_get("name")?,
            definition: row.try_get::<String, _>("definition")?,
            materialized: false,
            columns: Vec::new(), // resolved lazily by the data grid in later phases
        });
    }
    Ok(views)
}

async fn introspect_routines(pool: &MySqlPool, db: &str) -> Result<Vec<Function>> {
    let rows = sqlx::query(
        "SELECT routine_name AS name, routine_type AS kind,
                external_language AS language,
                dtd_identifier AS return_type,
                routine_definition AS definition
         FROM information_schema.routines
         WHERE routine_schema = ?
         ORDER BY routine_name",
    )
    .bind(db)
    .fetch_all(pool)
    .await?;

    let mut functions = Vec::with_capacity(rows.len());
    for row in rows {
        let kind: String = row.try_get("kind")?;
        functions.push(Function {
            name: row.try_get("name")?,
            kind: if kind.eq_ignore_ascii_case("PROCEDURE") {
                FunctionKind::Procedure
            } else {
                FunctionKind::Function
            },
            language: row
                .try_get::<Option<String>, _>("language")?
                .unwrap_or_else(|| "SQL".into()),
            args: Vec::new(), // information_schema lacks signatures; SHOW CREATE in Phase 2
            return_type: row
                .try_get::<Option<String>, _>("return_type")?
                .unwrap_or_default(),
            definition: row
                .try_get::<Option<String>, _>("definition")?
                .unwrap_or_default(),
            is_strict: false,
            volatility: None,
        });
    }
    Ok(functions)
}

/// Map one MySQL column value to the driver-agnostic [`Value`].
fn mysql_value(row: &MySqlRow, i: usize) -> Result<Value> {
    let raw = row.try_get_raw(i).map_err(decode_error)?;
    if raw.is_null() {
        return Ok(Value::Null);
    }
    let type_name = row
        .columns()
        .get(i)
        .map(|c| c.type_info())
        .map(|t| t.name().to_string())
        .unwrap_or_else(|| "UNKNOWN".into());

    macro_rules! get {
        ($ty:ty) => {
            row.try_get::<$ty, _>(i).map_err(decode_error)?
        };
    }

    let value = match type_name.as_str() {
        "TINY" | "YEAR" => Value::Int(i64::from(get!(i8))),
        "SHORT" => Value::Int(i64::from(get!(i16))),
        "INT24" | "LONG" => Value::Int(i64::from(get!(i32))),
        "LONGLONG" => match row.try_get::<i64, _>(i) {
            Ok(v) => Value::Int(v),
            Err(_) => Value::UInt(get!(u64)), // BIGINT UNSIGNED
        },
        "FLOAT" => Value::Float(f64::from(get!(f32))),
        "DOUBLE" => Value::Float(get!(f64)),
        "DECIMAL" | "NEWDECIMAL" => Value::Numeric(get!(rust_decimal::Decimal).to_string()),
        "BIT" => Value::Int(get!(i64)),
        "TINY_BLOB" | "BLOB" | "MEDIUM_BLOB" | "LONG_BLOB" | "BINARY" | "VAR_BINARY" => {
            Value::Bytes(get!(Vec<u8>))
        }
        "JSON" => Value::Json(get!(serde_json::Value).to_string()),
        "DATE" => Value::Datetime(get!(chrono::NaiveDate).to_string()),
        "TIME" => Value::Datetime(get!(chrono::NaiveTime).to_string()),
        "DATETIME" | "TIMESTAMP" => Value::Datetime(get!(chrono::NaiveDateTime).to_string()),
        "VARCHAR" | "STRING" | "VAR_STRING" | "ENUM" | "SET" => Value::Text(get!(String)),
        "NULL" => Value::Null,
        _ => match row.try_get::<String, _>(i) {
            Ok(s) => Value::Text(s),
            Err(_) => unsupported(&type_name),
        },
    };
    Ok(value)
}
