//! SQLite provider: connection, catalog introspection via `sqlite_master`
//! and PRAGMAs, and query execution via `sqlx`.

use std::time::Instant;

use async_trait::async_trait;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions, SqliteRow};
use sqlx::{Column as _, Row as _, TypeInfo as _, ValueRef as _};

use crate::conn::ConnectionProfile;
use crate::error::{MyDbError, Result};
use crate::introspect::*;
use crate::provider::{DatabaseProvider, ProviderKind};
use crate::providers::{
    column_meta, decode_error, is_row_returning, timed, unsupported, ACQUIRE_TIMEOUT,
    MAX_CONNECTIONS,
};
use crate::query::{QueryResult, Value};

/// Connected (or connectable) SQLite database file.
pub struct SqliteProvider {
    profile: ConnectionProfile,
    pool: Option<SqlitePool>,
}

impl SqliteProvider {
    /// Create an unconnected provider for a profile.
    pub fn new(profile: ConnectionProfile) -> Self {
        Self {
            profile,
            pool: None,
        }
    }

    fn pool(&self) -> Result<&SqlitePool> {
        self.pool
            .as_ref()
            .ok_or_else(|| MyDbError::Database("not connected".into()))
    }
}

/// Quote an identifier for use in PRAGMA statements (which take no binds).
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

#[async_trait]
impl DatabaseProvider for SqliteProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Sqlite
    }

    async fn connect(&mut self) -> Result<()> {
        let path = self
            .profile
            .database
            .clone()
            .or_else(|| {
                self.profile
                    .url
                    .as_ref()
                    .and_then(|u| u.strip_prefix("sqlite:"))
                    .map(|p| p.split('?').next().unwrap_or(p).to_string())
            })
            .ok_or_else(|| {
                MyDbError::InvalidValue(format!(
                    "connection '{}' is sqlite but has no database path",
                    self.profile.name
                ))
            })?;
        let options: SqliteConnectOptions = path
            .parse::<SqliteConnectOptions>()
            .map_err(|e| MyDbError::InvalidValue(format!("invalid sqlite path '{path}': {e}")))?
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(MAX_CONNECTIONS)
            .acquire_timeout(ACQUIRE_TIMEOUT)
            .connect_with(options)
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
        let database = self
            .profile
            .database
            .clone()
            .unwrap_or_else(|| "main".to_string());
        let mut catalog = SchemaCatalog::new(database, ProviderKind::Sqlite);
        catalog.captured_at = Some(chrono::Utc::now().to_rfc3339());

        let mut schema = Schema::new("main");

        // Tables
        let table_rows = sqlx::query(
            "SELECT name, sql FROM sqlite_master
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
             ORDER BY name",
        )
        .fetch_all(pool)
        .await?;
        for row in table_rows {
            let name: String = row.try_get("name")?;
            let create_sql: Option<String> = row.try_get("sql")?;
            let mut table = Table {
                name: name.clone(),
                ..Default::default()
            };
            introspect_sqlite_table(pool, &name, &mut table).await?;
            if let Some(sql) = create_sql.as_deref() {
                table.checks = parse_sqlite_checks(sql);
            }
            schema.tables.push(table);
        }

        // Views
        let view_rows =
            sqlx::query("SELECT name, sql FROM sqlite_master WHERE type = 'view' ORDER BY name")
                .fetch_all(pool)
                .await?;
        for row in view_rows {
            schema.views.push(View {
                name: row.try_get("name")?,
                definition: row.try_get::<Option<String>, _>("sql")?.unwrap_or_default(),
                materialized: false,
                columns: Vec::new(),
            });
        }

        // Triggers (attached to tables)
        let trigger_rows = sqlx::query(
            "SELECT name, tbl_name, sql FROM sqlite_master WHERE type = 'trigger' ORDER BY name",
        )
        .fetch_all(pool)
        .await?;
        for row in trigger_rows {
            let table_name: String = row.try_get("tbl_name")?;
            let sql: Option<String> = row.try_get("sql")?;
            let definition = sql.unwrap_or_default();
            let (timing, events, orientation) = parse_sqlite_trigger(&definition);
            if let Some(t) = schema.tables.iter_mut().find(|t| t.name == table_name) {
                t.triggers.push(Trigger {
                    name: row.try_get("name")?,
                    timing,
                    events,
                    orientation,
                    condition: None,
                    definition,
                });
            }
        }

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
                .map(|i| sqlite_value(row, i))
                .collect::<Result<Vec<_>>>()?;
            result.rows.push(values);
        }
        result.duration = timed(start);
        Ok(result)
    }

    async fn server_version(&self) -> Result<String> {
        let pool = self.pool()?;
        let version: String = sqlx::query_scalar("SELECT sqlite_version()")
            .fetch_one(pool)
            .await?;
        Ok(format!("SQLite {version}"))
    }
}

async fn introspect_sqlite_table(pool: &SqlitePool, name: &str, table: &mut Table) -> Result<()> {
    let quoted = quote_ident(name);

    // Columns + primary key (table_info: cid, name, type, notnull, dflt_value, pk)
    let col_rows = sqlx::query(&format!("PRAGMA table_info({quoted})"))
        .fetch_all(pool)
        .await?;
    let mut pk_columns = Vec::new();
    for row in col_rows {
        let ordinal: i64 = row.try_get("cid")?;
        let col_name: String = row.try_get("name")?;
        let data_type: String = row.try_get("type")?;
        let notnull: i64 = row.try_get("notnull")?;
        let default: Option<String> = row.try_get("dflt_value")?;
        let pk: i64 = row.try_get("pk")?;
        if pk > 0 {
            pk_columns.push((pk, col_name.clone()));
        }
        let is_identity = pk > 0 && default.is_none(); // INTEGER PRIMARY KEY = rowid alias
        table.columns.push(Column {
            name: col_name,
            ordinal: (ordinal + 1) as u32,
            data_type,
            nullable: notnull == 0,
            default,
            is_identity,
            identity_generation: (pk > 0).then(|| "BY DEFAULT".to_string()),
            identity_start: None,
            identity_increment: None,
            generated_expr: None,
            comment: None,
        });
    }
    if !pk_columns.is_empty() {
        pk_columns.sort_by_key(|(pos, _)| *pos);
        table.primary_key = Some(PrimaryKey {
            name: Some(format!("sqlite_autoindex_{name}_pk")),
            columns: pk_columns.into_iter().map(|(_, c)| c).collect(),
        });
    }

    // Foreign keys (id, seq, table, from, to, on_update, on_delete, match)
    let fk_rows = sqlx::query(&format!("PRAGMA foreign_key_list({quoted})"))
        .fetch_all(pool)
        .await?;
    let mut fks: Vec<ForeignKey> = Vec::new();
    for row in fk_rows {
        let id: i64 = row.try_get("id")?;
        let from: String = row.try_get("from")?;
        let to: Option<String> = row.try_get("to")?;
        let ref_table: String = row.try_get("table")?;
        let on_update: Option<String> = row.try_get("on_update")?;
        let on_delete: Option<String> = row.try_get("on_delete")?;
        let name = format!("fk_{name}_{id}");
        match fks.iter_mut().find(|fk| fk.name == name) {
            Some(fk) => {
                fk.columns.push(from);
                if let Some(to) = to {
                    fk.ref_columns.push(to);
                }
            }
            None => fks.push(ForeignKey {
                name,
                columns: vec![from],
                ref_schema: "main".into(),
                ref_table,
                ref_columns: to.into_iter().collect(),
                on_delete,
                on_update,
            }),
        }
    }
    table.foreign_keys = fks;

    // Indexes (seq, name, unique, origin, partial)
    let idx_rows = sqlx::query(&format!("PRAGMA index_list({quoted})"))
        .fetch_all(pool)
        .await?;
    for row in idx_rows {
        let idx_name: String = row.try_get("name")?;
        let unique: i64 = row.try_get("unique")?;
        let origin: String = row.try_get("origin")?;
        if origin == "pk" {
            continue; // modelled as the primary key
        }
        let col_rows = sqlx::query(&format!("PRAGMA index_info({})", quote_ident(&idx_name)))
            .fetch_all(pool)
            .await?;
        let columns: Vec<String> = col_rows
            .iter()
            .map(|r| {
                r.try_get::<Option<String>, _>("name")
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| "<expr>".into())
            })
            .collect();
        table.indexes.push(Index {
            name: idx_name,
            columns,
            unique: unique == 1,
            method: Some("btree".into()),
            partial: None,
        });
    }

    Ok(())
}

/// Extract CHECK constraints from a `CREATE TABLE` statement.
fn parse_sqlite_checks(sql: &str) -> Vec<CheckConstraint> {
    let mut checks = Vec::new();
    let upper = sql.to_ascii_uppercase();
    let mut search_from = 0;
    let mut unnamed = 0;
    while let Some(rel) = upper[search_from..].find("CHECK") {
        let abs = search_from + rel;
        let after = abs + 5;
        let rest = sql[after..].trim_start();
        if !rest.starts_with('(') {
            search_from = after;
            continue;
        }
        let expr_start = sql.len() - rest.len();
        if let Some(expr) = balanced_parens(&sql[expr_start..]) {
            unnamed += 1;
            let name = named_constraint_before(&sql[..abs], unnamed);
            checks.push(CheckConstraint {
                name,
                expression: expr.to_string(),
            });
            search_from = expr_start + expr.len();
        } else {
            break;
        }
    }
    checks
}

fn named_constraint_before(prefix: &str, unnamed: usize) -> String {
    let trimmed = prefix.trim_end();
    let upper = trimmed.to_ascii_uppercase();
    if let Some(idx) = upper.rfind("CONSTRAINT") {
        let after = trimmed[idx + "CONSTRAINT".len()..].trim();
        let name: String = after
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '"')
            .collect();
        let name = name.trim_matches('"');
        if !name.is_empty() {
            return name.to_string();
        }
    }
    format!("check_{unnamed}")
}

fn balanced_parens(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'(') {
        return None;
    }
    let mut depth = 0i32;
    let mut in_str = None;
    for (i, &b) in bytes.iter().enumerate() {
        if let Some(q) = in_str {
            if b == q {
                in_str = None;
            }
            continue;
        }
        match b {
            b'\'' | b'"' => in_str = Some(b),
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Best-effort parse of `CREATE TRIGGER` SQL for timing/events/orientation.
fn parse_sqlite_trigger(sql: &str) -> (String, Vec<String>, String) {
    let upper = sql.to_uppercase();
    let timing = if upper.contains("INSTEAD OF") {
        "INSTEAD OF"
    } else if upper.contains("BEFORE") {
        "BEFORE"
    } else {
        "AFTER"
    };
    let mut events = Vec::new();
    if upper.contains("INSERT") {
        events.push("INSERT".to_string());
    }
    if upper.contains("UPDATE") {
        events.push("UPDATE".to_string());
    }
    if upper.contains("DELETE") {
        events.push("DELETE".to_string());
    }
    let orientation = if upper.contains("FOR EACH ROW") {
        "ROW"
    } else {
        "STATEMENT"
    };
    (timing.into(), events, orientation.into())
}

/// Map one SQLite column value to the driver-agnostic [`Value`].
fn sqlite_value(row: &SqliteRow, i: usize) -> Result<Value> {
    let raw = row.try_get_raw(i).map_err(decode_error)?;
    if raw.is_null() {
        return Ok(Value::Null);
    }
    let type_name = row
        .columns()
        .get(i)
        .map(|c| c.type_info())
        .map(|t| t.name().to_string())
        .unwrap_or_else(|| "TEXT".into());

    let value = match type_name.as_str() {
        "NULL" => Value::Null,
        "BOOLEAN" => Value::Bool(row.try_get::<bool, _>(i).map_err(decode_error)?),
        "INTEGER" => Value::Int(row.try_get::<i64, _>(i).map_err(decode_error)?),
        "FLOAT" | "REAL" | "NUMERIC" => {
            Value::Float(row.try_get::<f64, _>(i).map_err(decode_error)?)
        }
        "BLOB" => Value::Bytes(row.try_get::<Vec<u8>, _>(i).map_err(decode_error)?),
        // TEXT, DATETIME, and declared types SQLite stores as text.
        _ => match row.try_get::<String, _>(i) {
            Ok(s) => Value::Text(s),
            Err(_) => unsupported(&type_name),
        },
    };
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_inline_and_named_checks() {
        let sql = "CREATE TABLE orders (
            id INTEGER PRIMARY KEY,
            total REAL NOT NULL CHECK (total >= 0),
            CONSTRAINT orders_note_chk CHECK (note IS NULL OR length(note) < 100)
        )";
        let checks = parse_sqlite_checks(sql);
        assert_eq!(checks.len(), 2);
        assert!(checks[0].expression.contains("total >= 0"));
        assert_eq!(checks[1].name, "orders_note_chk");
    }
}
