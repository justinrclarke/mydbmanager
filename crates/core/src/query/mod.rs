//! Ad-hoc query execution results.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// A single result value, driver-agnostic so it serializes cleanly to the
/// desktop UI and to CSV/JSON in the CLI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Value {
    /// SQL NULL.
    Null,
    /// Boolean.
    Bool(bool),
    /// Signed integer (fits i64).
    Int(i64),
    /// Unsigned integer (MySQL BIGINT UNSIGNED).
    UInt(u64),
    /// Floating point.
    Float(f64),
    /// Exact numeric kept as string to avoid precision loss (NUMERIC/DECIMAL).
    Numeric(String),
    /// Text.
    Text(String),
    /// Raw bytes (BYTEA/BLOB).
    Bytes(Vec<u8>),
    /// JSON, kept as raw text.
    Json(String),
    /// UUID, canonical string form.
    Uuid(String),
    /// Date/time, RFC 3339 or engine-native string form.
    Datetime(String),
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Null => f.write_str("NULL"),
            Value::Bool(v) => write!(f, "{v}"),
            Value::Int(v) => write!(f, "{v}"),
            Value::UInt(v) => write!(f, "{v}"),
            Value::Float(v) => write!(f, "{v}"),
            Value::Numeric(v) => f.write_str(v),
            Value::Text(v) => f.write_str(v),
            Value::Bytes(v) => write!(f, "<{} bytes>", v.len()),
            Value::Json(v) => f.write_str(v),
            Value::Uuid(v) => f.write_str(v),
            Value::Datetime(v) => f.write_str(v),
        }
    }
}

/// Metadata for one result column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnMeta {
    /// Column name or alias.
    pub name: String,
    /// Engine-reported type name.
    pub type_name: String,
}

/// The full result of an ad-hoc query.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QueryResult {
    /// Column metadata in select order.
    pub columns: Vec<ColumnMeta>,
    /// Rows of values, aligned with `columns`.
    pub rows: Vec<Vec<Value>>,
    /// Rows affected for non-SELECT statements.
    pub rows_affected: u64,
    /// Server + transfer duration.
    #[serde(with = "duration_millis")]
    pub duration: Duration,
}

mod duration_millis {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(d.as_millis().min(u64::MAX as u128) as u64)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        Ok(Duration::from_millis(u64::deserialize(d)?))
    }
}
