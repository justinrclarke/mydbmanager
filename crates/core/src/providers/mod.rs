//! Concrete [`DatabaseProvider`](crate::DatabaseProvider) implementations and
//! the connection factory.
//!
//! Connections go through an optional [`crate::ssh::SshTunnel`] when the
//! profile has `ssh` configured.

mod mysql;
mod postgres;
mod sqlite;

pub use mysql::MysqlProvider;
pub use postgres::PostgresProvider;
pub use sqlite::SqliteProvider;

use url::Url;

use crate::conn::ConnectionProfile;
use crate::error::{MyDbError, Result};
use crate::introspect::SchemaCatalog;
use crate::provider::{DatabaseProvider, ProviderKind};
use crate::query::{ColumnMeta, QueryResult, Value};
use crate::ssh::{self, SshTunnel};
use std::time::{Duration, Instant};
/// Acquire timeout for CLI-style short-lived pools.
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(10);
/// Pool size: small — one interactive session at a time.
const MAX_CONNECTIONS: u32 = 4;

/// Build a connection URL for a profile (explicit `url` wins over the
/// discrete host/port/user/database fields).
pub fn build_url(profile: &ConnectionProfile) -> Result<String> {
    if let Some(url) = &profile.url {
        return Ok(url.clone());
    }
    match profile.kind {
        ProviderKind::Sqlite => {
            let path = profile.database.as_deref().ok_or_else(|| {
                MyDbError::InvalidValue(format!(
                    "connection '{}' is sqlite but has no database path",
                    profile.name
                ))
            })?;
            Ok(format!("sqlite://{}?mode=rwc", path))
        }
        kind => {
            let host = profile.host.clone().ok_or_else(|| {
                MyDbError::InvalidValue(format!("connection '{}' has no host", profile.name))
            })?;
            let database = profile.database.clone().unwrap_or_default();
            let port = profile.port.unwrap_or_else(|| kind.default_port());
            let mut url = Url::parse(&format!("{kind}://{host}:{port}/{database}"))
                .map_err(|e| MyDbError::InvalidValue(format!("cannot build URL: {e}")))?;
            if let Some(user) = &profile.user {
                url.set_username(user)
                    .map_err(|_| MyDbError::InvalidValue("invalid user in URL".into()))?;
            }
            if let Some(password) = &profile.password {
                url.set_password(Some(password))
                    .map_err(|_| MyDbError::InvalidValue("invalid password in URL".into()))?;
            }
            Ok(url.to_string())
        }
    }
}

/// Provider plus optional SSH tunnel (must outlive the sqlx pool).
struct LiveConnection {
    inner: Box<dyn DatabaseProvider>,
    _tunnel: Option<SshTunnel>,
}

#[async_trait::async_trait]
impl DatabaseProvider for LiveConnection {
    fn kind(&self) -> ProviderKind {
        self.inner.kind()
    }

    async fn connect(&mut self) -> Result<()> {
        self.inner.connect().await
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.inner.disconnect().await
    }

    async fn introspect(&self) -> Result<SchemaCatalog> {
        self.inner.introspect().await
    }

    async fn execute_query(&self, sql: &str) -> Result<QueryResult> {
        self.inner.execute_query(sql).await
    }

    async fn server_version(&self) -> Result<String> {
        self.inner.server_version().await
    }
}

/// Connect to the engine described by `profile`, returning a ready-to-use
/// provider. Opens an SSH tunnel first when `profile.ssh` is set.
pub async fn connect(profile: &ConnectionProfile) -> Result<Box<dyn DatabaseProvider>> {
    let mut working = profile.clone();
    let tunnel = if profile.ssh.is_some() && profile.kind != ProviderKind::Sqlite {
        let ssh = profile.ssh.clone().unwrap();
        let (host, port) = ssh::destination(profile)?;
        let tunnel = ssh::open_tunnel(&ssh, &host, port).await?;
        working = ssh::rewrite_through_local(working, tunnel.local_addr);
        Some(tunnel)
    } else {
        None
    };
    let mut provider: Box<dyn DatabaseProvider> = match working.kind {
        ProviderKind::Postgres => Box::new(PostgresProvider::new(working)),
        ProviderKind::Mysql => Box::new(MysqlProvider::new(working)),
        ProviderKind::Sqlite => Box::new(SqliteProvider::new(working)),
    };
    provider.connect().await?;
    Ok(Box::new(LiveConnection {
        inner: provider,
        _tunnel: tunnel,
    }))
}

/// Build [`QueryResult`] metadata scaffolding shared by all providers.
pub(crate) fn timed(start: Instant) -> Duration {
    start.elapsed()
}

/// Column metadata helper shared by providers.
pub(crate) fn column_meta(name: impl Into<String>, type_name: impl Into<String>) -> ColumnMeta {
    ColumnMeta {
        name: name.into(),
        type_name: type_name.into(),
    }
}

/// Fallback when a driver type has no explicit mapping.
pub(crate) fn unsupported(type_name: &str) -> Value {
    Value::Text(format!("<unsupported type: {type_name}>"))
}

/// Map a decode failure to the engine error type.
pub(crate) fn decode_error(err: impl std::fmt::Display) -> MyDbError {
    MyDbError::Database(format!("cannot decode result value: {err}"))
}

/// True when `sql` is expected to return a row set (use `fetch_all`).
pub(crate) fn is_row_returning(sql: &str) -> bool {
    let trimmed = strip_leading_sql_comments(sql);
    let upper = trimmed.to_ascii_uppercase();
    upper.starts_with("SELECT")
        || upper.starts_with("WITH")
        || upper.starts_with("SHOW")
        || upper.starts_with("PRAGMA")
        || upper.starts_with("EXPLAIN")
        || upper.starts_with("VALUES")
        || upper.starts_with("TABLE ")
        || upper.contains(" RETURNING ")
}

fn strip_leading_sql_comments(sql: &str) -> &str {
    let mut s = sql.trim_start();
    loop {
        if s.starts_with("--") {
            s = s
                .split_once('\n')
                .map(|(_, rest)| rest)
                .unwrap_or("")
                .trim_start();
            continue;
        }
        if s.starts_with("/*") {
            s = s
                .split_once("*/")
                .map(|(_, rest)| rest)
                .unwrap_or("")
                .trim_start();
            continue;
        }
        break;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(mut p: ConnectionProfile) -> ConnectionProfile {
        if p.name.is_empty() {
            p.name = "test".into();
        }
        p
    }

    #[test]
    fn explicit_url_wins() {
        let p = profile(ConnectionProfile {
            kind: ProviderKind::Postgres,
            url: Some("postgres://u@h/db".into()),
            host: Some("ignored".into()),
            ..Default::default()
        });
        assert_eq!(build_url(&p).unwrap(), "postgres://u@h/db");
    }

    #[test]
    fn builds_postgres_url_from_parts() {
        let p = profile(ConnectionProfile {
            kind: ProviderKind::Postgres,
            host: Some("db.example.com".into()),
            user: Some("app".into()),
            password: Some("s3cr3t&more".into()),
            database: Some("appdb".into()),
            ..Default::default()
        });
        let url = build_url(&p).unwrap();
        assert_eq!(url, "postgres://app:s3cr3t&more@db.example.com:5432/appdb");
        // round-trips through the URL parser with escaping intact
        let parsed = Url::parse(&url).unwrap();
        assert_eq!(parsed.password(), Some("s3cr3t&more"));
    }

    #[test]
    fn builds_sqlite_url_from_path() {
        let p = profile(ConnectionProfile {
            kind: ProviderKind::Sqlite,
            database: Some("/tmp/mydb/test.sqlite".into()),
            ..Default::default()
        });
        assert_eq!(
            build_url(&p).unwrap(),
            "sqlite:///tmp/mydb/test.sqlite?mode=rwc"
        );
    }

    #[test]
    fn sqlite_requires_path() {
        let p = profile(ConnectionProfile {
            kind: ProviderKind::Sqlite,
            ..Default::default()
        });
        assert!(build_url(&p).is_err());
    }
}
