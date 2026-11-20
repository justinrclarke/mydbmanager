//! Hybrid dump/restore engine.
//!
//! Strategy: detect the vendor tools (`pg_dump`/`pg_restore`/`mysqldump`)
//! and use them when available for maximum fidelity; otherwise fall back to
//! the native Rust engine driven by the [`SchemaCatalog`](crate::introspect::SchemaCatalog),
//! which emits DDL in dependency order (types → sequences → tables →
//! constraints → indexes → views → functions → triggers) and restores
//! sequence values with `setval()` after data load.

mod native;
mod tools;

pub use native::{dump_sql, objects_to_create, restore_sql, schema_sql, table_copy_order};
pub use tools::{
    choose_dump_backend, detect as detect_tools, dump_with_tool, restore_backend_for,
    restore_with_tool, DetectedTool,
};

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::conn::ConnectionProfile;
use crate::error::{MyDbError, Result};
use crate::provider::{DatabaseProvider, ProviderKind};
use crate::transfer::{TransferEvent, TransferEventKind, VerificationReport};

/// Output format of a dump.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DumpFormat {
    /// `pg_dump -Fc` custom format (Postgres) / engine-native archive.
    #[default]
    Custom,
    /// Plain SQL script, portable everywhere.
    Sql,
    /// Directory format (`pg_dump -Fd`), parallel-restorable.
    Directory,
}

impl DumpFormat {
    /// Canonical name.
    pub fn as_str(self) -> &'static str {
        match self {
            DumpFormat::Custom => "custom",
            DumpFormat::Sql => "sql",
            DumpFormat::Directory => "directory",
        }
    }
}

impl std::fmt::Display for DumpFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for DumpFormat {
    type Err = crate::MyDbError;

    fn from_str(s: &str) -> crate::Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "custom" | "c" | "fc" => Ok(DumpFormat::Custom),
            "sql" | "plain" | "p" => Ok(DumpFormat::Sql),
            "dir" | "directory" | "d" | "fd" => Ok(DumpFormat::Directory),
            other => Err(crate::MyDbError::InvalidValue(format!(
                "unknown dump format '{other}' (expected custom, sql or directory)"
            ))),
        }
    }
}

/// Which implementation runs a dump/restore.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolBackend {
    /// Built-in Rust engine (always available).
    Native,
    /// `pg_dump` (Postgres).
    PgDump,
    /// `pg_restore` (Postgres).
    PgRestore,
    /// `mysqldump` (MySQL/MariaDB).
    MySqlDump,
    /// `mysql` client for restores (MySQL/MariaDB).
    MySql,
}

impl ToolBackend {
    /// Canonical name.
    pub fn as_str(self) -> &'static str {
        match self {
            ToolBackend::Native => "native",
            ToolBackend::PgDump => "pg_dump",
            ToolBackend::PgRestore => "pg_restore",
            ToolBackend::MySqlDump => "mysqldump",
            ToolBackend::MySql => "mysql",
        }
    }
}

impl std::fmt::Display for ToolBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ToolBackend {
    type Err = MyDbError;

    fn from_str(s: &str) -> crate::Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "native" | "builtin" | "rust" => Ok(ToolBackend::Native),
            "pg_dump" | "pgdump" => Ok(ToolBackend::PgDump),
            "pg_restore" | "pgrestore" => Ok(ToolBackend::PgRestore),
            "mysqldump" => Ok(ToolBackend::MySqlDump),
            "mysql" => Ok(ToolBackend::MySql),
            other => Err(MyDbError::InvalidValue(format!(
                "unknown backend '{other}'"
            ))),
        }
    }
}

/// Options controlling what gets dumped.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct DumpOptions {
    /// Output format.
    pub format: DumpFormat,
    /// Destination file/directory; None = stdout (CLI) or temp dir (engine).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<PathBuf>,
    /// Restrict to these schemas (empty = all).
    pub schemas: Vec<String>,
    /// Restrict to these tables (empty = all).
    pub tables: Vec<String>,
    /// Tables to skip.
    pub exclude_tables: Vec<String>,
    /// DDL only, no data.
    pub schema_only: bool,
    /// Data only, no DDL.
    pub data_only: bool,
    /// Include DROP statements before each CREATE (clean restore).
    pub clean: bool,
    /// Skip sequence definitions *and* their values (off by default — the
    /// whole point is that sequences are never skipped).
    pub skip_sequences: bool,
    /// Force a specific backend instead of auto-detecting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<ToolBackend>,
}

/// Options controlling how a dump is restored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct RestoreOptions {
    /// Drop existing objects before recreating them.
    pub clean: bool,
    /// Only drop objects that actually exist (`--if-exists`).
    pub if_exists: bool,
    /// Restore as the connecting user, not the original owner.
    pub no_owner: bool,
    /// Create the target database if missing.
    pub create_database: bool,
    /// Parallel restore workers (directory format).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jobs: Option<u8>,
    /// Report the plan without executing anything.
    pub dry_run: bool,
    /// Skip post-restore verification against a source catalog.
    pub skip_verify: bool,
}

/// Dump `provider` according to `options`. Returns the output path (empty if stdout).
pub async fn dump(
    profile: &ConnectionProfile,
    provider: &dyn DatabaseProvider,
    options: DumpOptions,
    emit: impl Fn(TransferEvent),
) -> Result<String> {
    let job_id = uuid::Uuid::new_v4().to_string();
    emit(TransferEvent::info(
        &job_id,
        TransferEventKind::Started,
        format!("dumping '{}'", profile.name),
    ));
    let backend = tools::choose_dump_backend(profile, &options).await;
    emit(TransferEvent::info(
        &job_id,
        TransferEventKind::Planning,
        format!("backend: {backend}"),
    ));

    if profile.kind == ProviderKind::Sqlite && options.format == DumpFormat::Custom {
        if let (Some(out), Some(src)) = (&options.output, &profile.database) {
            if !options.schema_only {
                let dest = out.display().to_string().replace('\'', "''");
                let _ = src;
                provider
                    .execute_query(&format!("VACUUM INTO '{dest}'"))
                    .await?;
                emit(TransferEvent::info(
                    &job_id,
                    TransferEventKind::Finished,
                    format!("wrote {}", out.display()),
                ));
                return Ok(out.display().to_string());
            }
        }
    }

    let use_tool = matches!(backend, ToolBackend::PgDump | ToolBackend::MySqlDump)
        && options.output.is_some()
        && options.format != DumpFormat::Sql;
    if use_tool {
        let out = options.output.as_ref().unwrap();
        tools::dump_with_tool(profile, &options, backend, out).await?;
        emit(TransferEvent::info(
            &job_id,
            TransferEventKind::Finished,
            format!("wrote {}", out.display()),
        ));
        return Ok(out.display().to_string());
    }

    emit(TransferEvent::info(
        &job_id,
        TransferEventKind::Introspecting,
        "reading catalog",
    ));
    let catalog = provider.introspect().await?;
    let sql = native::dump_sql(provider, &catalog, &options, &emit, &job_id).await?;
    if let Some(out) = &options.output {
        if options.format == DumpFormat::Directory {
            tokio::fs::create_dir_all(out).await?;
            tokio::fs::write(out.join("dump.sql"), sql.as_bytes()).await?;
        } else {
            if let Some(parent) = out.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::write(out, sql.as_bytes()).await?;
        }
        Ok(out.display().to_string())
    } else {
        Ok(sql)
    }
}

/// Restore `input` into `provider`.
pub async fn restore(
    profile: &ConnectionProfile,
    provider: &dyn DatabaseProvider,
    input: &Path,
    options: RestoreOptions,
    emit: impl Fn(TransferEvent),
) -> Result<usize> {
    let job_id = uuid::Uuid::new_v4().to_string();
    emit(TransferEvent::info(
        &job_id,
        TransferEventKind::Started,
        format!("restoring into '{}'", profile.name),
    ));
    let backend = tools::restore_backend_for(input, profile.kind);
    if matches!(backend, ToolBackend::PgRestore | ToolBackend::MySql) && !options.dry_run {
        tools::restore_with_tool(profile, input, &options, backend).await?;
        emit(TransferEvent::info(
            &job_id,
            TransferEventKind::Finished,
            "restore complete",
        ));
        return Ok(0);
    }
    let script = if input.is_dir() {
        tokio::fs::read_to_string(input.join("dump.sql")).await?
    } else {
        tokio::fs::read_to_string(input).await?
    };
    let n = native::restore_sql(provider, &script, &options, &emit, &job_id).await?;
    emit(TransferEvent::info(
        &job_id,
        TransferEventKind::Finished,
        format!("executed {n} statements"),
    ));
    Ok(n)
}

/// Compare two connected databases (object counts, row counts, sequences).
pub async fn verify_providers(
    source: &dyn DatabaseProvider,
    target: &dyn DatabaseProvider,
) -> Result<VerificationReport> {
    let src = source.introspect().await?;
    let dst = target.introspect().await?;
    let src_rows = row_counts(source, &src).await?;
    let dst_rows = row_counts(target, &dst).await?;
    Ok(VerificationReport::compare(
        &src, &dst, &src_rows, &dst_rows,
    ))
}

/// Per-table `COUNT(*)` as `(schema.table, count)`.
pub async fn row_counts(
    provider: &dyn DatabaseProvider,
    catalog: &crate::introspect::SchemaCatalog,
) -> Result<Vec<(String, u64)>> {
    let kind = catalog.provider;
    let mut out = Vec::new();
    for schema in &catalog.schemas {
        for table in &schema.tables {
            let q = kind.qualify(&schema.name, &table.name);
            let result = provider
                .execute_query(&format!("SELECT COUNT(*) AS n FROM {q}"))
                .await?;
            let n = result
                .rows
                .first()
                .and_then(|r| r.first())
                .map(|v| match v {
                    crate::query::Value::Int(i) => *i as u64,
                    crate::query::Value::UInt(u) => *u,
                    crate::query::Value::Numeric(s) => s.parse().unwrap_or(0),
                    crate::query::Value::Text(s) => s.parse().unwrap_or(0),
                    _ => 0,
                })
                .unwrap_or(0);
            out.push((format!("{}.{}", schema.name, table.name), n));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dump_format_parses() {
        assert_eq!("custom".parse::<DumpFormat>().unwrap(), DumpFormat::Custom);
        assert_eq!("sql".parse::<DumpFormat>().unwrap(), DumpFormat::Sql);
        assert_eq!("dir".parse::<DumpFormat>().unwrap(), DumpFormat::Directory);
        assert!("tar".parse::<DumpFormat>().is_err());
    }

    #[test]
    fn dump_options_default_keeps_sequences() {
        let opts = DumpOptions::default();
        assert!(!opts.skip_sequences);
        assert_eq!(opts.format, DumpFormat::Custom);
    }
}
