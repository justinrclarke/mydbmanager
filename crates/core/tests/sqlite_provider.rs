//! Integration tests for the SQLite provider against a temp-file database.
//! These run everywhere (no server needed).

use std::path::{Path, PathBuf};

use mydb_core::conn::ConnectionProfile;
use mydb_core::providers;
use mydb_core::query::Value;
use mydb_core::ProviderKind;

fn temp_db(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "mydb-sqlite-test-{}-{}.sqlite",
        std::process::id(),
        name
    ));
    let _ = std::fs::remove_file(&path);
    path
}

fn profile_for(path: &Path) -> ConnectionProfile {
    ConnectionProfile {
        name: "sqlitetest".into(),
        kind: ProviderKind::Sqlite,
        database: Some(path.to_string_lossy().to_string()),
        ..Default::default()
    }
}

const FIXTURE: &[&str] = &[
    "CREATE TABLE users (
        id INTEGER PRIMARY KEY,
        email TEXT NOT NULL UNIQUE,
        balance REAL DEFAULT 0,
        created_at TEXT DEFAULT (datetime('now'))
    )",
    "CREATE TABLE orders (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        total REAL NOT NULL CHECK (total >= 0),
        note TEXT
    )",
    "CREATE INDEX orders_user_idx ON orders(user_id)",
    "CREATE VIEW active_users AS SELECT id, email FROM users WHERE balance > 0",
    "CREATE TRIGGER orders_touch AFTER UPDATE ON orders FOR EACH ROW
     BEGIN UPDATE users SET balance = balance WHERE id = NEW.user_id; END",
    "INSERT INTO users (id, email, balance) VALUES (1, 'a@example.com', 10.5)",
    "INSERT INTO users (id, email, balance) VALUES (2, 'b@example.com', 0)",
    "INSERT INTO orders (user_id, total) VALUES (1, 99.99)",
];

async fn seeded_provider(path: &Path) -> Box<dyn mydb_core::DatabaseProvider> {
    let provider = providers::connect(&profile_for(path))
        .await
        .expect("connect");
    for stmt in FIXTURE {
        provider
            .execute_query(stmt)
            .await
            .unwrap_or_else(|e| panic!("fixture statement failed: {e}\n---\n{stmt}"));
    }
    provider
}

#[tokio::test]
async fn introspects_full_sqlite_catalog() {
    let path = temp_db("catalog");
    let mut provider = seeded_provider(&path).await;

    let catalog = provider.introspect().await.expect("introspect");
    assert_eq!(catalog.provider, ProviderKind::Sqlite);
    assert!(catalog.captured_at.is_some());

    let schema = catalog.find_schema("main").expect("main schema");
    assert_eq!(schema.tables.len(), 2);
    assert!(schema.views.iter().any(|v| v.name == "active_users"));

    let users = schema.find_table("users").unwrap();
    assert_eq!(users.columns.len(), 4);
    let id = &users.columns[0];
    assert_eq!(id.name, "id");
    assert!(id.is_identity);
    let email = users.columns.iter().find(|c| c.name == "email").unwrap();
    assert!(!email.nullable);
    assert!(users.primary_key.is_some());
    assert_eq!(users.primary_key.as_ref().unwrap().columns, vec!["id"]);
    // UNIQUE email shows up as an auto index
    assert!(users.indexes.iter().any(|i| i.unique));

    let orders = schema.find_table("orders").unwrap();
    assert_eq!(orders.foreign_keys.len(), 1);
    let fk = &orders.foreign_keys[0];
    assert_eq!(fk.ref_table, "users");
    assert_eq!(fk.columns, vec!["user_id"]);
    assert_eq!(fk.ref_columns, vec!["id"]);
    assert_eq!(fk.on_delete.as_deref(), Some("CASCADE"));
    assert_eq!(orders.checks.len(), 1);
    assert!(orders.checks[0].expression.contains("total"));
    assert!(orders.indexes.iter().any(|i| i.name == "orders_user_idx"));
    assert_eq!(orders.triggers.len(), 1);
    let trigger = &orders.triggers[0];
    assert_eq!(trigger.name, "orders_touch");
    assert_eq!(trigger.timing, "AFTER");
    assert_eq!(trigger.events, vec!["UPDATE".to_string()]);
    assert_eq!(trigger.orientation, "ROW");

    // SQLite capabilities: no sequences/functions as catalog objects
    assert!(schema.sequences.is_empty());
    assert!(schema.functions.is_empty());

    provider.disconnect().await.unwrap();
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn executes_queries_with_value_mapping() {
    let path = temp_db("query");
    let mut provider = seeded_provider(&path).await;

    let result = provider
        .execute_query("SELECT id, email, balance, NULL AS nul FROM users ORDER BY id")
        .await
        .expect("query");
    assert_eq!(result.columns.len(), 4);
    assert_eq!(result.columns[0].name, "id");
    assert_eq!(result.rows.len(), 2);
    assert_eq!(result.rows[0][0], Value::Int(1));
    assert_eq!(result.rows[0][1], Value::Text("a@example.com".into()));
    assert_eq!(result.rows[0][2], Value::Float(10.5));
    assert_eq!(result.rows[0][3], Value::Null);

    let version = provider.server_version().await.expect("version");
    assert!(version.starts_with("SQLite "), "got: {version}");

    provider.disconnect().await.unwrap();
    let _ = std::fs::remove_file(&path);
}
