//! Integration tests for the MySQL provider against a real server.
//! Skipped unless `MYDB_TEST_MYSQL_URL` points at a disposable database
//! (e.g. the Docker container from AGENTS.md).

use mydb_core::conn::ConnectionProfile;
use mydb_core::introspect::FunctionKind;
use mydb_core::providers;
use mydb_core::query::Value;
use mydb_core::{DatabaseProvider, ProviderKind};

fn mysql_url() -> Option<String> {
    std::env::var("MYDB_TEST_MYSQL_URL")
        .ok()
        .filter(|s| !s.is_empty())
}

fn profile(url: &str) -> ConnectionProfile {
    ConnectionProfile {
        name: "mysqltest".into(),
        kind: ProviderKind::Mysql,
        url: Some(url.into()),
        ..Default::default()
    }
}

const FIXTURE: &[&str] = &[
    "DROP VIEW IF EXISTS active_users",
    "DROP TRIGGER IF EXISTS orders_shout",
    "DROP FUNCTION IF EXISTS discount",
    "DROP TABLE IF EXISTS orders",
    "DROP TABLE IF EXISTS users",
    "CREATE TABLE users (
        id BIGINT AUTO_INCREMENT PRIMARY KEY,
        email VARCHAR(255) NOT NULL,
        status ENUM('active','disabled') NOT NULL DEFAULT 'active',
        balance DECIMAL(10,2) DEFAULT 0.00,
        metadata JSON NULL,
        created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
        UNIQUE KEY users_email_uq (email)
    ) COMMENT='app users'",
    "CREATE TABLE orders (
        id BIGINT AUTO_INCREMENT PRIMARY KEY,
        user_id BIGINT NOT NULL,
        total DECIMAL(12,2) NOT NULL,
        note VARCHAR(255) NULL,
        CONSTRAINT orders_total_chk CHECK (total >= 0),
        CONSTRAINT orders_user_fk FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
    )",
    "CREATE INDEX orders_user_idx ON orders (user_id)",
    "CREATE VIEW active_users AS SELECT id, email FROM users WHERE status = 'active'",
    "CREATE FUNCTION discount(x DECIMAL(10,2)) RETURNS DECIMAL(10,2) DETERMINISTIC NO SQL
     RETURN x * 0.9",
    "CREATE TRIGGER orders_shout BEFORE UPDATE ON orders
     FOR EACH ROW SET NEW.note = UPPER(COALESCE(NEW.note, ''))",
    "INSERT INTO users (email, balance, metadata) VALUES
        ('a@example.com', 10.50, '{\"plan\": \"pro\"}'),
        ('b@example.com', 0.00, NULL)",
    "INSERT INTO orders (user_id, total, note) VALUES (1, 99.99, 'gift')",
];

async fn seeded_provider() -> Option<Box<dyn DatabaseProvider>> {
    let url = mysql_url()?;
    let provider = providers::connect(&profile(&url)).await.expect("connect");
    for stmt in FIXTURE {
        provider
            .execute_query(stmt)
            .await
            .unwrap_or_else(|e| panic!("fixture statement failed: {e}\n---\n{stmt}"));
    }
    Some(provider)
}

#[tokio::test]
async fn introspects_full_mysql_catalog() {
    let Some(mut provider) = seeded_provider().await else {
        eprintln!("skipping: MYDB_TEST_MYSQL_URL not set");
        return;
    };

    let catalog = provider.introspect().await.expect("introspect");
    assert_eq!(catalog.provider, ProviderKind::Mysql);
    assert_eq!(
        catalog.schemas.len(),
        1,
        "MySQL: one schema == the database"
    );
    let schema = &catalog.schemas[0];
    assert_eq!(schema.name, catalog.database);

    // --- tables & columns ---
    assert_eq!(schema.tables.len(), 2);
    let users = schema.find_table("users").unwrap();
    assert_eq!(users.comment.as_deref(), Some("app users"));
    assert_eq!(users.columns.len(), 6);
    let id = users.columns.iter().find(|c| c.name == "id").unwrap();
    assert!(id.is_identity, "AUTO_INCREMENT detected");
    assert!(id.data_type.contains("bigint"));
    let status = users.columns.iter().find(|c| c.name == "status").unwrap();
    assert!(
        status.data_type.starts_with("enum("),
        "enum column type kept verbatim: {}",
        status.data_type
    );

    // --- constraints ---
    let orders = schema.find_table("orders").unwrap();
    assert!(users.primary_key.is_some());
    assert_eq!(orders.foreign_keys.len(), 1);
    let fk = &orders.foreign_keys[0];
    assert_eq!(fk.name, "orders_user_fk");
    assert_eq!(fk.ref_table, "users");
    assert_eq!(fk.ref_columns, vec!["id"]);
    assert_eq!(fk.on_delete.as_deref(), Some("CASCADE"));
    assert!(
        orders.checks.iter().any(|c| c.expression.contains("total")),
        "CHECK constraint captured (MySQL 8.0.16+)"
    );

    // --- indexes ---
    assert!(orders
        .indexes
        .iter()
        .any(|i| i.name == "orders_user_idx" && !i.unique));
    assert!(users
        .indexes
        .iter()
        .any(|i| i.name == "users_email_uq" && i.unique));

    // --- views, functions, triggers ---
    assert!(schema
        .views
        .iter()
        .any(|v| v.name == "active_users" && !v.definition.is_empty()));
    let func = schema
        .functions
        .iter()
        .find(|f| f.name == "discount")
        .expect("function");
    assert_eq!(func.kind, FunctionKind::Function);
    assert!(func.definition.contains("0.9"));
    let trigger = orders
        .triggers
        .iter()
        .find(|t| t.name == "orders_shout")
        .expect("trigger");
    assert_eq!(trigger.timing, "BEFORE");
    assert_eq!(trigger.events, vec!["UPDATE".to_string()]);
    assert_eq!(trigger.orientation, "ROW");

    // --- capability-aware: MySQL has no standalone sequences ---
    assert!(schema.sequences.is_empty());

    provider.disconnect().await.unwrap();
}

#[tokio::test]
async fn maps_mysql_value_types() {
    let Some(mut provider) = seeded_provider().await else {
        eprintln!("skipping: MYDB_TEST_MYSQL_URL not set");
        return;
    };

    let result = provider
        .execute_query(
            "SELECT id, email, balance, status, metadata,
                    CAST('2026-11-20' AS DATE) AS d, NULL AS nul
             FROM users ORDER BY id",
        )
        .await
        .expect("query");
    assert_eq!(result.rows.len(), 2);
    let row = &result.rows[0];
    assert_eq!(row[0], Value::Int(1));
    assert_eq!(row[1], Value::Text("a@example.com".into()));
    assert_eq!(row[2], Value::Numeric("10.50".into()));
    assert_eq!(row[3], Value::Text("active".into()));
    match &row[4] {
        Value::Json(j) => assert!(j.contains("pro")),
        other => panic!("expected JSON, got {other:?}"),
    }
    assert!(matches!(&row[5], Value::Datetime(d) if d.starts_with("2026-11-20")));
    assert_eq!(row[6], Value::Null);

    let version = provider.server_version().await.expect("version");
    assert!(version.starts_with("MySQL "), "got: {version}");

    provider.disconnect().await.unwrap();
}
