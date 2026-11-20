//! PostgreSQL provider: connection, full catalog introspection and query
//! execution via `sqlx`.

use std::time::Instant;

use async_trait::async_trait;
use sqlx::postgres::{PgPool, PgPoolOptions, PgRow};
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

/// Connected (or connectable) PostgreSQL database.
pub struct PostgresProvider {
    profile: ConnectionProfile,
    pool: Option<PgPool>,
}

impl PostgresProvider {
    /// Create an unconnected provider for a profile.
    pub fn new(profile: ConnectionProfile) -> Self {
        Self {
            profile,
            pool: None,
        }
    }

    fn pool(&self) -> Result<&PgPool> {
        self.pool
            .as_ref()
            .ok_or_else(|| MyDbError::Database("not connected".into()))
    }
}

#[async_trait]
impl DatabaseProvider for PostgresProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Postgres
    }

    async fn connect(&mut self) -> Result<()> {
        let url = build_url(&self.profile)?;
        let pool = PgPoolOptions::new()
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
        let database: String = sqlx::query_scalar("SELECT current_database()")
            .fetch_one(pool)
            .await?;
        let mut catalog = SchemaCatalog::new(database, ProviderKind::Postgres);
        catalog.captured_at = Some(chrono::Utc::now().to_rfc3339());

        let schema_names: Vec<String> = sqlx::query_scalar(
            "SELECT nspname FROM pg_namespace
             WHERE nspname NOT IN ('pg_catalog', 'information_schema')
               AND nspname NOT LIKE 'pg_%'
             ORDER BY nspname",
        )
        .fetch_all(pool)
        .await?;

        for name in schema_names {
            let mut schema = Schema::new(&name);
            schema.tables = introspect_tables(pool, &name).await?;
            attach_constraints(pool, &name, &mut schema.tables).await?;
            attach_indexes(pool, &name, &mut schema.tables).await?;
            attach_triggers(pool, &name, &mut schema.tables).await?;
            schema.sequences = introspect_sequences(pool, &name).await?;
            schema.views = introspect_views(pool, &name, false).await?;
            schema.materialized_views = introspect_views(pool, &name, true).await?;
            schema.functions = introspect_functions(pool, &name).await?;
            schema.enums = introspect_enums(pool, &name).await?;
            if name == "public" {
                schema.extensions = introspect_extensions(pool).await?;
            }
            catalog.schemas.push(schema);
        }
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
                .map(|i| pg_value(row, i))
                .collect::<Result<Vec<_>>>()?;
            result.rows.push(values);
        }
        result.duration = timed(start);
        Ok(result)
    }

    async fn server_version(&self) -> Result<String> {
        let pool = self.pool()?;
        let version: String = sqlx::query_scalar("SHOW server_version")
            .fetch_one(pool)
            .await?;
        Ok(format!("PostgreSQL {version}"))
    }
}

async fn introspect_tables(pool: &PgPool, schema: &str) -> Result<Vec<Table>> {
    let rows = sqlx::query(
        "SELECT c.relname AS name,
                obj_description(c.oid) AS comment,
                CASE WHEN c.reltuples >= 0 THEN c.reltuples::bigint END AS row_estimate
         FROM pg_class c
         JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = $1 AND c.relkind IN ('r', 'p')
         ORDER BY c.relname",
    )
    .bind(schema)
    .fetch_all(pool)
    .await?;

    let mut tables = Vec::with_capacity(rows.len());
    for row in rows {
        let name: String = row.try_get("name")?;
        tables.push(Table {
            name: name.clone(),
            comment: row.try_get("comment").unwrap_or(None),
            row_estimate: row
                .try_get::<Option<i64>, _>("row_estimate")
                .unwrap_or(None)
                .map(|v| v.max(0) as u64),
            columns: introspect_columns(pool, schema, &name).await?,
            ..Default::default()
        });
    }
    Ok(tables)
}

async fn introspect_columns(pool: &PgPool, schema: &str, table: &str) -> Result<Vec<Column>> {
    let rows = sqlx::query(
        "SELECT a.attname AS name,
                a.attnum AS ordinal,
                format_type(a.atttypid, a.atttypmod) AS data_type,
                NOT a.attnotnull AS nullable,
                pg_get_expr(d.adbin, d.adrelid) AS default_expr,
                a.attidentity AS identity,
                a.attgenerated AS generated,
                col_description(a.attrelid, a.attnum) AS comment
         FROM pg_attribute a
         JOIN pg_class c ON c.oid = a.attrelid
         JOIN pg_namespace n ON n.oid = c.relnamespace
         LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
         WHERE n.nspname = $1 AND c.relname = $2 AND a.attnum > 0 AND NOT a.attisdropped
         ORDER BY a.attnum",
    )
    .bind(schema)
    .bind(table)
    .fetch_all(pool)
    .await?;

    let mut columns = Vec::with_capacity(rows.len());
    for row in rows {
        let identity: Option<String> = row.try_get("identity")?;
        let generated: Option<String> = row.try_get("generated")?;
        let default_expr: Option<String> = row.try_get("default_expr")?;
        let is_generated = generated.as_deref() == Some("s");
        columns.push(Column {
            name: row.try_get("name")?,
            ordinal: row.try_get::<i32, _>("ordinal")? as u32,
            data_type: row.try_get("data_type")?,
            nullable: row.try_get("nullable")?,
            default: if is_generated {
                None
            } else {
                default_expr.clone()
            },
            is_identity: identity.as_deref().is_some_and(|i| !i.is_empty()),
            identity_generation: match identity.as_deref() {
                Some("a") => Some("ALWAYS".into()),
                Some("d") => Some("BY DEFAULT".into()),
                _ => None,
            },
            identity_start: None,
            identity_increment: None,
            generated_expr: if is_generated { default_expr } else { None },
            comment: row.try_get("comment")?,
        });
    }
    Ok(columns)
}

async fn attach_constraints(pool: &PgPool, schema: &str, tables: &mut [Table]) -> Result<()> {
    // Primary keys
    let pks = sqlx::query(
        "SELECT c.relname AS table_name,
                con.conname AS name,
                (SELECT array_agg(a.attname ORDER BY k.ord)
                 FROM unnest(con.conkey) WITH ORDINALITY AS k(attnum, ord)
                 JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.attnum
                ) AS columns
         FROM pg_constraint con
         JOIN pg_class c ON c.oid = con.conrelid
         JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE con.contype = 'p' AND n.nspname = $1",
    )
    .bind(schema)
    .fetch_all(pool)
    .await?;
    for row in pks {
        let table: String = row.try_get("table_name")?;
        if let Some(t) = tables.iter_mut().find(|t| t.name == table) {
            t.primary_key = Some(PrimaryKey {
                name: row.try_get("name")?,
                columns: row.try_get::<Vec<String>, _>("columns")?,
            });
        }
    }

    // Foreign keys
    let fks = sqlx::query(
        "SELECT c.relname AS table_name,
                con.conname AS name,
                (SELECT array_agg(a.attname ORDER BY k.ord)
                 FROM unnest(con.conkey) WITH ORDINALITY AS k(attnum, ord)
                 JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.attnum
                ) AS columns,
                rn.nspname AS ref_schema,
                rc.relname AS ref_table,
                (SELECT array_agg(a.attname ORDER BY k.ord)
                 FROM unnest(con.confkey) WITH ORDINALITY AS k(attnum, ord)
                 JOIN pg_attribute a ON a.attrelid = con.confrelid AND a.attnum = k.attnum
                ) AS ref_columns,
                con.confdeltype AS on_delete,
                con.confupdtype AS on_update
         FROM pg_constraint con
         JOIN pg_class c ON c.oid = con.conrelid
         JOIN pg_namespace n ON n.oid = c.relnamespace
         JOIN pg_class rc ON rc.oid = con.confrelid
         JOIN pg_namespace rn ON rn.oid = rc.relnamespace
         WHERE con.contype = 'f' AND n.nspname = $1",
    )
    .bind(schema)
    .fetch_all(pool)
    .await?;
    for row in fks {
        let table: String = row.try_get("table_name")?;
        if let Some(t) = tables.iter_mut().find(|t| t.name == table) {
            let del: String = row.try_get("on_delete")?;
            let upd: String = row.try_get("on_update")?;
            t.foreign_keys.push(ForeignKey {
                name: row.try_get("name")?,
                columns: row.try_get("columns")?,
                ref_schema: row.try_get("ref_schema")?,
                ref_table: row.try_get("ref_table")?,
                ref_columns: row.try_get("ref_columns")?,
                on_delete: Some(fk_action(&del)),
                on_update: Some(fk_action(&upd)),
            });
        }
    }

    // CHECK constraints (skip implicit NOT NULL checks on PG18+)
    let checks = sqlx::query(
        "SELECT c.relname AS table_name,
                con.conname AS name,
                pg_get_constraintdef(con.oid) AS expression
         FROM pg_constraint con
         JOIN pg_class c ON c.oid = con.conrelid
         JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE con.contype = 'c' AND n.nspname = $1
           AND pg_get_constraintdef(con.oid) NOT LIKE 'NOT NULL%'
         ORDER BY con.conname",
    )
    .bind(schema)
    .fetch_all(pool)
    .await?;
    for row in checks {
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

fn fk_action(code: &str) -> String {
    match code {
        "a" => "NO ACTION".into(),
        "r" => "RESTRICT".into(),
        "c" => "CASCADE".into(),
        "n" => "SET NULL".into(),
        "d" => "SET DEFAULT".into(),
        other => other.to_uppercase(),
    }
}

async fn attach_indexes(pool: &PgPool, schema: &str, tables: &mut [Table]) -> Result<()> {
    let rows = sqlx::query(
        "SELECT t.relname AS table_name,
                ic.relname AS name,
                i.indisunique AS is_unique,
                am.amname AS method,
                pg_get_expr(i.indpred, i.indrelid) AS partial_predicate,
                (SELECT array_agg(COALESCE(a.attname, pg_get_indexdef(i.indexrelid)) ORDER BY k.ord)
                 FROM unnest(i.indkey) WITH ORDINALITY AS k(attnum, ord)
                 LEFT JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = k.attnum
                ) AS columns
         FROM pg_index i
         JOIN pg_class ic ON ic.oid = i.indexrelid
         JOIN pg_class t ON t.oid = i.indrelid
         JOIN pg_namespace n ON n.oid = t.relnamespace
         JOIN pg_am am ON am.oid = ic.relam
         WHERE n.nspname = $1
         ORDER BY ic.relname",
    )
    .bind(schema)
    .fetch_all(pool)
    .await?;

    for row in rows {
        let table: String = row.try_get("table_name")?;
        if let Some(t) = tables.iter_mut().find(|t| t.name == table) {
            t.indexes.push(Index {
                name: row.try_get("name")?,
                columns: row.try_get("columns")?,
                unique: row.try_get("is_unique")?,
                method: row.try_get("method")?,
                partial: row.try_get("partial_predicate")?,
            });
        }
    }
    Ok(())
}

async fn attach_triggers(pool: &PgPool, schema: &str, tables: &mut [Table]) -> Result<()> {
    let rows = sqlx::query(
        "SELECT c.relname AS table_name,
                tg.tgname AS name,
                tg.tgtype::int AS tgtype,
                pg_get_expr(tg.tgqual, tg.tgrelid) AS condition,
                pg_get_triggerdef(tg.oid) AS definition
         FROM pg_trigger tg
         JOIN pg_class c ON c.oid = tg.tgrelid
         JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = $1 AND NOT tg.tgisinternal
         ORDER BY tg.tgname",
    )
    .bind(schema)
    .fetch_all(pool)
    .await?;

    for row in rows {
        let table: String = row.try_get("table_name")?;
        let tgtype: i32 = row.try_get("tgtype")?;
        let timing = if tgtype & 64 != 0 {
            "INSTEAD OF"
        } else if tgtype & 2 != 0 {
            "BEFORE"
        } else {
            "AFTER"
        };
        let mut events = Vec::new();
        if tgtype & 4 != 0 {
            events.push("INSERT".to_string());
        }
        if tgtype & 8 != 0 {
            events.push("DELETE".to_string());
        }
        if tgtype & 16 != 0 {
            events.push("UPDATE".to_string());
        }
        if tgtype & 32 != 0 {
            events.push("TRUNCATE".to_string());
        }
        if let Some(t) = tables.iter_mut().find(|t| t.name == table) {
            t.triggers.push(Trigger {
                name: row.try_get("name")?,
                timing: timing.into(),
                events,
                orientation: if tgtype & 1 != 0 { "ROW" } else { "STATEMENT" }.into(),
                condition: row.try_get("condition")?,
                definition: row.try_get("definition")?,
            });
        }
    }
    Ok(())
}

async fn introspect_sequences(pool: &PgPool, schema: &str) -> Result<Vec<Sequence>> {
    let rows = sqlx::query(
        "SELECT s.sequencename AS name,
                s.data_type,
                s.start_value::bigint AS start,
                s.increment::bigint AS increment,
                s.minimum_value::bigint AS min,
                s.maximum_value::bigint AS max,
                s.cache_size::bigint AS cache,
                s.cycle,
                s.last_value,
                dep.table_name AS owned_table,
                dep.column_name AS owned_column
         FROM pg_sequences s
         LEFT JOIN (
             SELECT sn.nspname AS seq_schema,
                    sc.relname AS seq_name,
                    t.relname AS table_name,
                    a.attname AS column_name
             FROM pg_depend d
             JOIN pg_class sc ON sc.oid = d.objid
             JOIN pg_namespace sn ON sn.oid = sc.relnamespace
             JOIN pg_class t ON t.oid = d.refobjid
             JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = d.refobjsubid
             WHERE sc.relkind = 'S' AND d.deptype IN ('a', 'i')
         ) dep ON dep.seq_schema = s.schemaname AND dep.seq_name = s.sequencename
         WHERE s.schemaname = $1
         ORDER BY s.sequencename",
    )
    .bind(schema)
    .fetch_all(pool)
    .await?;

    let mut sequences = Vec::with_capacity(rows.len());
    for row in rows {
        let owned: Option<(String, String)> = match (
            row.try_get::<Option<String>, _>("owned_table")?,
            row.try_get::<Option<String>, _>("owned_column")?,
        ) {
            (Some(t), Some(c)) => Some((t, c)),
            _ => None,
        };
        sequences.push(Sequence {
            name: row.try_get("name")?,
            data_type: row.try_get("data_type")?,
            start: row.try_get("start")?,
            increment: row.try_get("increment")?,
            min: row.try_get::<i64, _>("min")? as i128,
            max: row.try_get::<i64, _>("max")? as i128,
            cache: row.try_get("cache")?,
            cycle: row.try_get("cycle")?,
            last_value: row.try_get::<Option<i64>, _>("last_value")?.map(i128::from),
            owned_by: owned,
        });
    }
    Ok(sequences)
}

async fn introspect_views(pool: &PgPool, schema: &str, materialized: bool) -> Result<Vec<View>> {
    let rows = if materialized {
        sqlx::query(
            "SELECT matviewname AS name, definition
             FROM pg_matviews WHERE schemaname = $1 ORDER BY matviewname",
        )
        .bind(schema)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query(
            "SELECT viewname AS name, definition
             FROM pg_views WHERE schemaname = $1 ORDER BY viewname",
        )
        .bind(schema)
        .fetch_all(pool)
        .await?
    };

    let mut views = Vec::with_capacity(rows.len());
    for row in rows {
        let name: String = row.try_get("name")?;
        let columns: Vec<String> = sqlx::query_scalar(
            "SELECT a.attname
             FROM pg_attribute a
             JOIN pg_class c ON c.oid = a.attrelid
             JOIN pg_namespace n ON n.oid = c.relnamespace
             WHERE n.nspname = $1 AND c.relname = $2 AND a.attnum > 0 AND NOT a.attisdropped
             ORDER BY a.attnum",
        )
        .bind(schema)
        .bind(&name)
        .fetch_all(pool)
        .await?;
        views.push(View {
            name,
            definition: row.try_get("definition")?,
            materialized,
            columns,
        });
    }
    Ok(views)
}

async fn introspect_functions(pool: &PgPool, schema: &str) -> Result<Vec<Function>> {
    let rows = sqlx::query(
        "SELECT p.proname AS name,
                p.prokind::text AS kind,
                l.lanname AS language,
                pg_get_function_arguments(p.oid) AS args,
                pg_get_function_result(p.oid) AS return_type,
                pg_get_functiondef(p.oid) AS definition,
                p.proisstrict AS is_strict,
                p.provolatile::text AS volatility
         FROM pg_proc p
         JOIN pg_namespace n ON n.oid = p.pronamespace
         JOIN pg_language l ON l.oid = p.prolang
         WHERE n.nspname = $1
         ORDER BY p.proname",
    )
    .bind(schema)
    .fetch_all(pool)
    .await?;

    let mut functions = Vec::with_capacity(rows.len());
    for row in rows {
        let kind: String = row.try_get("kind")?;
        let args: String = row.try_get("args")?;
        let volatility: String = row.try_get("volatility")?;
        functions.push(Function {
            name: row.try_get("name")?,
            kind: if kind == "p" {
                FunctionKind::Procedure
            } else {
                FunctionKind::Function
            },
            language: row.try_get("language")?,
            args: if args.trim().is_empty() {
                Vec::new()
            } else {
                args.split(", ").map(str::to_string).collect()
            },
            return_type: row.try_get("return_type")?,
            definition: row.try_get("definition")?,
            is_strict: row.try_get("is_strict")?,
            volatility: Some(match volatility.as_str() {
                "i" => "immutable".into(),
                "s" => "stable".into(),
                _ => "volatile".into(),
            }),
        });
    }
    Ok(functions)
}

async fn introspect_enums(pool: &PgPool, schema: &str) -> Result<Vec<EnumType>> {
    let rows = sqlx::query(
        "SELECT t.typname AS name,
                array_agg(e.enumlabel ORDER BY e.enumsortorder) AS labels
         FROM pg_type t
         JOIN pg_namespace n ON n.oid = t.typnamespace
         JOIN pg_enum e ON e.enumtypid = t.oid
         WHERE n.nspname = $1 AND t.typtype = 'e'
         GROUP BY t.typname
         ORDER BY t.typname",
    )
    .bind(schema)
    .fetch_all(pool)
    .await?;

    let mut enums = Vec::with_capacity(rows.len());
    for row in rows {
        enums.push(EnumType {
            name: row.try_get("name")?,
            labels: row.try_get("labels")?,
        });
    }
    Ok(enums)
}

async fn introspect_extensions(pool: &PgPool) -> Result<Vec<Extension>> {
    let rows = sqlx::query(
        "SELECT extname AS name, extversion AS version
         FROM pg_extension WHERE extname <> 'plpgsql' ORDER BY extname",
    )
    .fetch_all(pool)
    .await?;
    let mut extensions = Vec::with_capacity(rows.len());
    for row in rows {
        extensions.push(Extension {
            name: row.try_get("name")?,
            version: row.try_get("version")?,
        });
    }
    Ok(extensions)
}

/// Map one Postgres column value to the driver-agnostic [`Value`].
fn pg_value(row: &PgRow, i: usize) -> Result<Value> {
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
        "BOOL" => Value::Bool(get!(bool)),
        "INT2" => Value::Int(i64::from(get!(i16))),
        "INT4" | "OID" => Value::Int(i64::from(get!(i32))),
        "INT8" => Value::Int(get!(i64)),
        "FLOAT4" => Value::Float(f64::from(get!(f32))),
        "FLOAT8" => Value::Float(get!(f64)),
        "NUMERIC" => Value::Numeric(get!(rust_decimal::Decimal).to_string()),
        "BYTEA" => Value::Bytes(get!(Vec<u8>)),
        "JSON" | "JSONB" => Value::Json(get!(serde_json::Value).to_string()),
        "UUID" => Value::Uuid(get!(uuid::Uuid).to_string()),
        "DATE" => Value::Datetime(get!(chrono::NaiveDate).to_string()),
        "TIME" => Value::Datetime(get!(chrono::NaiveTime).to_string()),
        "TIMESTAMP" => Value::Datetime(get!(chrono::NaiveDateTime).to_string()),
        "TIMESTAMPTZ" => Value::Datetime(get!(chrono::DateTime<chrono::Utc>).to_rfc3339()),
        "TEXT" | "VARCHAR" | "BPCHAR" | "NAME" | "UNKNOWN" | "XML" => Value::Text(get!(String)),
        // Enums, citext, inet, etc.: most text-like types decode as String.
        _ => match row.try_get::<String, _>(i) {
            Ok(s) => Value::Text(s),
            Err(_) => match row.try_get::<Vec<String>, _>(i) {
                Ok(arr) => Value::Json(serde_json::to_string(&arr).unwrap_or_default()),
                Err(_) => unsupported(&type_name),
            },
        },
    };
    Ok(value)
}
