//! Integration tests for the Postgres provider against a real server.
//! Skipped unless `MYDB_TEST_PG_URL` points at a disposable database
//! (e.g. the Docker container from AGENTS.md).

use mydb_core::conn::ConnectionProfile;
use mydb_core::introspect::FunctionKind;
use mydb_core::providers;
use mydb_core::query::Value;
use mydb_core::{DatabaseProvider, ProviderKind};

fn pg_url() -> Option<String> {
    std::env::var("MYDB_TEST_PG_URL")
        .ok()
        .filter(|s| !s.is_empty())
}

fn profile(url: &str) -> ConnectionProfile {
    ConnectionProfile {
        name: "pgtest".into(),
        kind: ProviderKind::Postgres,
        url: Some(url.into()),
        ..Default::default()
    }
}

const FIXTURE: &[&str] = &[
    "DROP SCHEMA public CASCADE",
    "CREATE SCHEMA public",
    "CREATE TYPE user_status AS ENUM ('active', 'disabled')",
    "CREATE TABLE users (
        id bigserial PRIMARY KEY,
        email text NOT NULL UNIQUE,
        status user_status NOT NULL DEFAULT 'active',
        balance numeric(10,2) DEFAULT 0,
        metadata jsonb,
        created_at timestamptz NOT NULL DEFAULT now()
    )",
    "COMMENT ON TABLE users IS 'app users'",
    "COMMENT ON COLUMN users.email IS 'login email'",
    "CREATE TABLE orders (
        id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
        user_id bigint NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        total numeric(12,2) NOT NULL CHECK (total >= 0),
        note text
    )",
    "CREATE INDEX orders_user_id_idx ON orders (user_id)",
    "CREATE UNIQUE INDEX orders_note_uidx ON orders (note) WHERE note IS NOT NULL",
    "CREATE VIEW active_users AS SELECT id, email FROM users WHERE status = 'active'",
    "CREATE MATERIALIZED VIEW user_order_counts AS
     SELECT u.id, count(o.id) AS orders
     FROM users u LEFT JOIN orders o ON o.user_id = u.id GROUP BY u.id",
    "CREATE FUNCTION shout_note() RETURNS trigger LANGUAGE plpgsql AS $$
     BEGIN NEW.note := upper(coalesce(NEW.note, '')); RETURN NEW; END $$",
    "CREATE TRIGGER orders_shout BEFORE UPDATE ON orders
     FOR EACH ROW EXECUTE FUNCTION shout_note()",
    "INSERT INTO users (email, balance, metadata) VALUES
        ('a@example.com', 10.5, '{\"plan\": \"pro\"}'),
        ('b@example.com', 0, NULL)",
    "INSERT INTO orders (user_id, total, note) VALUES (1, 99.99, 'gift')",
    "SELECT setval('users_id_seq', 100)",
];

async fn seeded_provider() -> Option<Box<dyn DatabaseProvider>> {
    let url = pg_url()?;
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
async fn introspects_full_postgres_catalog() {
    let Some(mut provider) = seeded_provider().await else {
        eprintln!("skipping: MYDB_TEST_PG_URL not set");
        return;
    };

    let catalog = provider.introspect().await.expect("introspect");
    assert_eq!(catalog.provider, ProviderKind::Postgres);

    let public = catalog.find_schema("public").expect("public schema");

    // --- enums ---
    let status = public
        .enums
        .iter()
        .find(|e| e.name == "user_status")
        .expect("enum");
    assert_eq!(
        status.labels,
        vec!["active".to_string(), "disabled".to_string()]
    );

    // --- tables & columns ---
    assert_eq!(public.tables.len(), 2);
    let users = public.find_table("users").unwrap();
    assert_eq!(users.comment.as_deref(), Some("app users"));
    assert_eq!(users.columns.len(), 6);
    let id = users.columns.iter().find(|c| c.name == "id").unwrap();
    assert_eq!(id.data_type, "bigint");
    assert!(
        id.default.as_deref().unwrap().contains("users_id_seq"),
        "serial default: {:?}",
        id.default
    );
    let email = users.columns.iter().find(|c| c.name == "email").unwrap();
    assert_eq!(email.comment.as_deref(), Some("login email"));
    assert!(!email.nullable);
    let status_col = users.columns.iter().find(|c| c.name == "status").unwrap();
    assert_eq!(status_col.data_type, "user_status");

    let orders = public.find_table("orders").unwrap();
    let orders_id = orders.columns.iter().find(|c| c.name == "id").unwrap();
    assert!(
        orders_id.is_identity,
        "GENERATED ALWAYS AS IDENTITY detected"
    );
    assert_eq!(orders_id.identity_generation.as_deref(), Some("ALWAYS"));

    // --- constraints ---
    assert!(users.primary_key.is_some());
    assert_eq!(orders.foreign_keys.len(), 1);
    let fk = &orders.foreign_keys[0];
    assert_eq!(fk.ref_table, "users");
    assert_eq!(fk.ref_columns, vec!["id"]);
    assert_eq!(fk.on_delete.as_deref(), Some("CASCADE"));
    assert_eq!(orders.checks.len(), 1);
    assert!(orders.checks[0].expression.contains("total"));

    // --- indexes (including the partial unique one) ---
    assert!(orders
        .indexes
        .iter()
        .any(|i| i.name == "orders_user_id_idx" && !i.unique));
    let partial = orders
        .indexes
        .iter()
        .find(|i| i.name == "orders_note_uidx")
        .expect("partial index");
    assert!(partial.unique);
    assert!(
        partial.partial.as_deref().unwrap().contains("note"),
        "partial predicate captured: {:?}",
        partial.partial
    );

    // --- sequences: THE regression this project exists for ---
    let users_seq = public
        .sequences
        .iter()
        .find(|s| s.name == "users_id_seq")
        .expect("users_id_seq must be introspected");
    assert_eq!(
        users_seq.last_value,
        Some(100),
        "sequence last_value must survive"
    );
    assert_eq!(
        users_seq.owned_by,
        Some(("users".to_string(), "id".to_string())),
        "serial ownership resolved"
    );
    assert!(
        public.sequences.iter().any(|s| s.name == "orders_id_seq"),
        "identity sequence present"
    );

    // --- views, matviews, functions, triggers ---
    assert!(public
        .views
        .iter()
        .any(|v| v.name == "active_users" && !v.definition.is_empty()));
    assert!(public
        .materialized_views
        .iter()
        .any(|v| v.name == "user_order_counts"));
    let func = public
        .functions
        .iter()
        .find(|f| f.name == "shout_note")
        .expect("function");
    assert_eq!(func.kind, FunctionKind::Function);
    assert_eq!(func.language, "plpgsql");
    assert!(
        func.definition.contains("CREATE"),
        "full CREATE FUNCTION captured"
    );
    let trigger = orders
        .triggers
        .iter()
        .find(|t| t.name == "orders_shout")
        .expect("trigger");
    assert_eq!(trigger.timing, "BEFORE");
    assert_eq!(trigger.events, vec!["UPDATE".to_string()]);
    assert_eq!(trigger.orientation, "ROW");

    // --- counts feed verification ---
    let counts = catalog.object_counts();
    assert_eq!(counts.tables, 2);
    assert_eq!(counts.sequences, 2);
    assert_eq!(counts.views, 1);
    assert_eq!(counts.materialized_views, 1);
    assert_eq!(counts.enums, 1);
    assert!(counts.triggers >= 1);

    provider.disconnect().await.unwrap();
}

#[tokio::test]
async fn maps_postgres_value_types() {
    let Some(mut provider) = seeded_provider().await else {
        eprintln!("skipping: MYDB_TEST_PG_URL not set");
        return;
    };

    let result = provider
        .execute_query(
            "SELECT 1::int2 AS i2, 2::int4 AS i4, 3::int8 AS i8, 4.5::float8 AS f8,
                    10.50::numeric(10,2) AS num, true AS b, 'text'::text AS t,
                    NULL::text AS nul, '{\"a\": 1}'::jsonb AS j,
                    '00000000-0000-0000-0000-000000000001'::uuid AS u,
                    '\\x0102'::bytea AS blob, 'active'::user_status AS en,
                    DATE '2026-11-20' AS d",
        )
        .await
        .expect("query");
    let row = &result.rows[0];
    assert_eq!(row[0], Value::Int(1));
    assert_eq!(row[1], Value::Int(2));
    assert_eq!(row[2], Value::Int(3));
    assert_eq!(row[3], Value::Float(4.5));
    assert_eq!(row[4], Value::Numeric("10.50".into()));
    assert_eq!(row[5], Value::Bool(true));
    assert_eq!(row[6], Value::Text("text".into()));
    assert_eq!(row[7], Value::Null);
    assert_eq!(row[8], Value::Json("{\"a\": 1}".into()));
    assert_eq!(
        row[9],
        Value::Uuid("00000000-0000-0000-0000-000000000001".into())
    );
    assert_eq!(row[10], Value::Bytes(vec![1, 2]));
    assert_eq!(
        row[11],
        Value::Text("active".into()),
        "enums decode as text"
    );
    assert_eq!(row[12], Value::Datetime("2026-11-20".into()));

    let version = provider.server_version().await.expect("version");
    assert!(version.starts_with("PostgreSQL "), "got: {version}");

    provider.disconnect().await.unwrap();
}
