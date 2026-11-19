//! Error types shared across the engine.

use thiserror::Error;

/// Convenience alias for results produced by the engine.
pub type Result<T> = std::result::Result<T, MyDbError>;

/// All errors surfaced by `mydb-core`.
#[derive(Debug, Error)]
pub enum MyDbError {
    /// A referenced connection profile does not exist.
    #[error("connection '{0}' not found")]
    ConnectionNotFound(String),

    /// A connection profile with the same name already exists.
    #[error("connection '{0}' already exists")]
    ConnectionExists(String),

    /// An error reported by the database server or driver.
    #[error("database error: {0}")]
    Database(String),

    /// The target provider does not support the requested operation.
    #[error("unsupported for this provider: {0}")]
    Unsupported(String),

    /// The feature is scaffolded but lands in a later phase.
    #[error("not yet implemented: {0}")]
    NotImplemented(String),

    /// A user-supplied value could not be parsed.
    #[error("invalid value: {0}")]
    InvalidValue(String),

    /// Configuration file problems (missing directory, invalid TOML, ...).
    #[error("config error: {0}")]
    Config(String),

    /// Filesystem or subprocess IO failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<sqlx::Error> for MyDbError {
    fn from(err: sqlx::Error) -> Self {
        MyDbError::Database(err.to_string())
    }
}
