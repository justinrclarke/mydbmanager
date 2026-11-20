//! Shared SQL helpers: literals, glob matching, statement splitting, PATH lookup.

use crate::provider::ProviderKind;
use crate::query::Value;
use std::path::PathBuf;

/// Render a [`Value`] as an engine-specific SQL literal.
pub fn sql_literal(kind: ProviderKind, value: &Value) -> String {
    match value {
        Value::Null => "NULL".into(),
        Value::Bool(b) => match kind {
            ProviderKind::Sqlite => {
                if *b {
                    "1".into()
                } else {
                    "0".into()
                }
            }
            _ => {
                if *b {
                    "TRUE".into()
                } else {
                    "FALSE".into()
                }
            }
        },
        Value::Int(i) => i.to_string(),
        Value::UInt(u) => u.to_string(),
        Value::Float(f) => {
            if f.is_finite() {
                f.to_string()
            } else {
                "NULL".into()
            }
        }
        Value::Numeric(s) => s.clone(),
        Value::Text(s) | Value::Uuid(s) | Value::Datetime(s) => quote_string(s),
        Value::Json(s) => quote_string(s),
        Value::Bytes(b) => match kind {
            ProviderKind::Postgres => format!("'\\x{}'::bytea", hex(b)),
            _ => format!("X'{}'", hex(b)),
        },
    }
}

/// Single-quote a string, doubling interior quotes.
pub fn quote_string(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Glob match with `*` and `?`. `*` matches any run of characters.
pub fn glob_match(pat: &str, candidate: &str) -> bool {
    glob_rec(pat.as_bytes(), candidate.as_bytes())
}

fn glob_rec(pat: &[u8], cand: &[u8]) -> bool {
    match (pat.first(), cand.first()) {
        (None, None) => true,
        (Some(b'*'), _) => {
            glob_rec(&pat[1..], cand) || (!cand.is_empty() && glob_rec(pat, &cand[1..]))
        }
        (Some(b'?'), Some(_)) => glob_rec(&pat[1..], &cand[1..]),
        (Some(p), Some(c)) if p == c => glob_rec(&pat[1..], &cand[1..]),
        _ => false,
    }
}

/// True when `table` (bare or `schema.table`) matches any exclude glob.
pub fn is_excluded(schema: &str, table: &str, globs: &[String]) -> bool {
    if globs.is_empty() {
        return false;
    }
    let qualified = format!("{schema}.{table}");
    globs
        .iter()
        .any(|g| glob_match(g, table) || glob_match(g, &qualified))
}

/// Split a SQL script into statements on `;`, ignoring those inside quotes.
pub fn split_statements(script: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut chars = script.chars().peekable();
    let mut in_str: Option<char> = None;
    while let Some(c) = chars.next() {
        if let Some(q) = in_str {
            buf.push(c);
            if c == q {
                if chars.peek() == Some(&q) {
                    buf.push(chars.next().unwrap());
                } else {
                    in_str = None;
                }
            }
            continue;
        }
        match c {
            '\'' | '"' | '`' => {
                in_str = Some(c);
                buf.push(c);
            }
            ';' => {
                let stmt = buf.trim().to_string();
                if !stmt.is_empty() {
                    out.push(stmt);
                }
                buf.clear();
            }
            _ => buf.push(c),
        }
    }
    let stmt = buf.trim().to_string();
    if !stmt.is_empty() {
        out.push(stmt);
    }
    out
}

/// Locate `name` (and `name.exe` on Windows) on `PATH`.
pub fn find_executable(name: &str) -> Option<PathBuf> {
    let names = if cfg!(windows) && !name.ends_with(".exe") {
        vec![format!("{name}.exe"), name.to_string()]
    } else {
        vec![name.to_string()]
    };
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for n in &names {
            let candidate = dir.join(n);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Default insert batch size used when [`crate::transfer::TransferOptions::batch_size`] is 0.
pub const DEFAULT_BATCH_SIZE: usize = 5000;

/// Effective batch size.
pub fn effective_batch_size(requested: usize) -> usize {
    if requested == 0 {
        DEFAULT_BATCH_SIZE
    } else {
        requested
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_star_and_question() {
        assert!(glob_match("audit_*", "audit_log"));
        assert!(!glob_match("audit_*", "users"));
        assert!(glob_match("user?", "users"));
        assert!(glob_match("public.users", "public.users"));
    }

    #[test]
    fn splits_ignoring_quoted_semicolons() {
        let stmts = split_statements("SELECT 'a;b';\nCREATE TABLE t (id int);");
        assert_eq!(stmts.len(), 2);
        assert!(stmts[0].contains("a;b"));
    }

    #[test]
    fn sql_literal_quotes_strings() {
        assert_eq!(
            sql_literal(ProviderKind::Postgres, &Value::Text("o'reilly".into())),
            "'o''reilly'"
        );
        assert_eq!(sql_literal(ProviderKind::Sqlite, &Value::Bool(true)), "1");
        assert_eq!(sql_literal(ProviderKind::Postgres, &Value::Null), "NULL");
    }
}
