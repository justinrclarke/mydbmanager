//! Hybrid dump/restore engine (Phase 2).
//!
//! Strategy: detect the vendor tools (`pg_dump`/`pg_restore`/`mysqldump`)
//! and use them when available for maximum fidelity; otherwise fall back to
//! the native Rust engine driven by the [`SchemaCatalog`](crate::introspect::SchemaCatalog),
//! which emits DDL in dependency order (types → sequences → tables →
//! constraints → indexes → views → functions → triggers) and restores
//! sequence values with `setval()` after data load.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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
