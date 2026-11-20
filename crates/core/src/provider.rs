//! The provider abstraction: one trait, many database engines.

use std::fmt;
use std::str::FromStr;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::{MyDbError, Result};
use crate::introspect::SchemaCatalog;
use crate::query::QueryResult;

/// Supported database engines.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    /// PostgreSQL
    #[default]
    Postgres,
    /// MySQL (and MariaDB)
    Mysql,
    /// SQLite
    Sqlite,
}

impl ProviderKind {
    /// Canonical lowercase name used in config files and on the command line.
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKind::Postgres => "postgres",
            ProviderKind::Mysql => "mysql",
            ProviderKind::Sqlite => "sqlite",
        }
    }

    /// Feature flags describing what the engine supports, so callers never
    /// assume sequences/schemas/etc. exist.
    pub fn capabilities(self) -> Capabilities {
        match self {
            ProviderKind::Postgres => Capabilities {
                multiple_databases: true,
                schemas: true,
                sequences: true,
                identity_columns: true,
                enums: true,
                materialized_views: true,
                functions: true,
                triggers: true,
                extensions: true,
                returning: true,
            },
            ProviderKind::Mysql => Capabilities {
                multiple_databases: true,
                schemas: false, // a "schema" is a database in MySQL
                sequences: false,
                identity_columns: true, // AUTO_INCREMENT
                enums: true,            // ENUM column type
                materialized_views: false,
                functions: true,
                triggers: true,
                extensions: false,
                returning: false,
            },
            ProviderKind::Sqlite => Capabilities {
                multiple_databases: false, // attached DBs ignored for now
                schemas: false,
                sequences: false, // sqlite_sequence handled internally
                identity_columns: true,
                enums: false,
                materialized_views: false,
                functions: false, // only built-in functions
                triggers: true,
                extensions: true, // loadable extensions
                returning: true,
            },
        }
    }

    /// Conventional default TCP port (0 for file-based engines).
    pub fn default_port(self) -> u16 {
        match self {
            ProviderKind::Postgres => 5432,
            ProviderKind::Mysql => 3306,
            ProviderKind::Sqlite => 0,
        }
    }

    /// Quote an identifier with the engine's delimiter.
    pub fn quote_ident(self, ident: &str) -> String {
        match self {
            ProviderKind::Mysql => format!("`{}`", ident.replace('`', "``")),
            ProviderKind::Postgres | ProviderKind::Sqlite => {
                format!("\"{}\"", ident.replace('"', "\"\""))
            }
        }
    }

    /// Schema-qualified quoted name (`schema.table`), or just the name on SQLite.
    pub fn qualify(self, schema: &str, name: &str) -> String {
        match self {
            ProviderKind::Sqlite => self.quote_ident(name),
            _ => format!("{}.{}", self.quote_ident(schema), self.quote_ident(name)),
        }
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ProviderKind {
    type Err = MyDbError;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "postgres" | "postgresql" | "pg" => Ok(ProviderKind::Postgres),
            "mysql" | "mariadb" => Ok(ProviderKind::Mysql),
            "sqlite" | "sqlite3" => Ok(ProviderKind::Sqlite),
            other => Err(MyDbError::InvalidValue(format!(
                "unknown database kind '{other}' (expected postgres, mysql or sqlite)"
            ))),
        }
    }
}

/// What a given engine supports. Used by the transfer/diff engines and the UI
/// to grey out or adapt features per provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Capabilities {
    /// Server hosts multiple databases (Postgres/MySQL) vs one file (SQLite).
    pub multiple_databases: bool,
    /// Schemas inside a database (Postgres only).
    pub schemas: bool,
    /// Standalone sequence objects.
    pub sequences: bool,
    /// Identity / auto-increment columns.
    pub identity_columns: bool,
    /// Enum types (Postgres) or ENUM columns (MySQL).
    pub enums: bool,
    /// Materialized views.
    pub materialized_views: bool,
    /// User-defined functions / stored procedures.
    pub functions: bool,
    /// Triggers.
    pub triggers: bool,
    /// Extensions (Postgres extensions / SQLite loadable extensions).
    pub extensions: bool,
    /// `RETURNING` clause support.
    pub returning: bool,
}

/// Every database engine implements this trait; higher layers only ever talk
/// to the trait, never to a concrete driver.
#[async_trait]
pub trait DatabaseProvider: Send + Sync {
    /// Which engine this provider implements.
    fn kind(&self) -> ProviderKind;

    /// Feature flags for this engine.
    fn capabilities(&self) -> Capabilities {
        self.kind().capabilities()
    }

    /// Establish the connection (through an SSH tunnel when configured).
    async fn connect(&mut self) -> Result<()>;

    /// Close the connection and release resources.
    async fn disconnect(&mut self) -> Result<()>;

    /// Build a full [`SchemaCatalog`] of the connected database.
    async fn introspect(&self) -> Result<SchemaCatalog>;

    /// Run an ad-hoc query and return its full result set.
    async fn execute_query(&self, sql: &str) -> Result<QueryResult>;

    /// Human-readable server name and version (used by `mydb conn test`).
    async fn server_version(&self) -> Result<String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_kind_roundtrip() {
        for kind in [
            ProviderKind::Postgres,
            ProviderKind::Mysql,
            ProviderKind::Sqlite,
        ] {
            assert_eq!(kind.as_str().parse::<ProviderKind>().unwrap(), kind);
        }
    }

    #[test]
    fn provider_kind_aliases() {
        assert_eq!(
            "postgresql".parse::<ProviderKind>().unwrap(),
            ProviderKind::Postgres
        );
        assert_eq!(
            "PG".parse::<ProviderKind>().unwrap(),
            ProviderKind::Postgres
        );
        assert_eq!(
            "mariadb".parse::<ProviderKind>().unwrap(),
            ProviderKind::Mysql
        );
        assert!("oracle".parse::<ProviderKind>().is_err());
    }

    #[test]
    fn postgres_supports_sequences() {
        assert!(ProviderKind::Postgres.capabilities().sequences);
        assert!(!ProviderKind::Mysql.capabilities().sequences);
        assert!(!ProviderKind::Sqlite.capabilities().sequences);
    }

    #[test]
    fn quotes_idents_per_engine() {
        assert_eq!(ProviderKind::Postgres.quote_ident("a\"b"), "\"a\"\"b\"");
        assert_eq!(ProviderKind::Mysql.quote_ident("a`b"), "`a``b`");
        assert_eq!(ProviderKind::Sqlite.qualify("main", "users"), "\"users\"");
        assert_eq!(
            ProviderKind::Postgres.qualify("public", "users"),
            "\"public\".\"users\""
        );
    }
}
