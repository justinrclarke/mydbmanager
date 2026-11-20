//! Native dump → restore → verify, plus a clean-recreate transfer, on SQLite.
//! Always runs (temp files, no server).

use std::path::{Path, PathBuf};

use mydb_core::conn::ConnectionProfile;
use mydb_core::dump::{self, DumpOptions, RestoreOptions};
use mydb_core::providers;
use mydb_core::transfer::{self, TransferMode, TransferOptions};
use mydb_core::ProviderKind;

fn temp_db(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "mydb-roundtrip-{}-{}.sqlite",
        std::process::id(),
        name
    ));
    let _ = std::fs::remove_file(&path);
    path
}

fn profile(path: &Path) -> ConnectionProfile {
    ConnectionProfile {
        name: path.file_stem().unwrap().to_string_lossy().into(),
        kind: ProviderKind::Sqlite,
        database: Some(path.to_string_lossy().into()),
        ..Default::default()
    }
}

const FIXTURE: &[&str] = &[
    "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL UNIQUE)",
    "CREATE TABLE orders (
        id INTEGER PRIMARY KEY,
        user_id INTEGER NOT NULL REFERENCES users(id),
        total REAL NOT NULL CHECK (total >= 0)
    )",
    "INSERT INTO users (id, email) VALUES (1, 'a@example.com'), (2, 'b@example.com')",
    "INSERT INTO orders (id, user_id, total) VALUES (1, 1, 9.5)",
];

async fn seed(path: &Path) -> Box<dyn mydb_core::DatabaseProvider> {
    let p = providers::connect(&profile(path)).await.expect("connect");
    for stmt in FIXTURE {
        p.execute_query(stmt).await.expect(stmt);
    }
    p
}

#[tokio::test]
async fn dump_restore_verify_sqlite() {
    let src_path = temp_db("src");
    let dst_path = temp_db("dst");
    let dump_path =
        std::env::temp_dir().join(format!("mydb-roundtrip-{}-dump.sql", std::process::id()));
    let _ = std::fs::remove_file(&dump_path);

    let src = seed(&src_path).await;
    let src_profile = profile(&src_path);
    dump::dump(
        &src_profile,
        src.as_ref(),
        DumpOptions {
            format: mydb_core::dump::DumpFormat::Sql,
            output: Some(dump_path.clone()),
            backend: Some(mydb_core::dump::ToolBackend::Native),
            ..Default::default()
        },
        |_| {},
    )
    .await
    .expect("dump");

    let dst_profile = profile(&dst_path);
    let dst = providers::connect(&dst_profile).await.expect("dst connect");
    dump::restore(
        &dst_profile,
        dst.as_ref(),
        &dump_path,
        RestoreOptions::default(),
        |_| {},
    )
    .await
    .expect("restore");

    let report = dump::verify_providers(src.as_ref(), dst.as_ref())
        .await
        .expect("verify");
    assert!(
        report.passed,
        "src={:?} dst={:?} rows={:?} seq={:?}",
        report.source_counts,
        report.target_counts,
        report.row_mismatches,
        report.sequence_mismatches
    );

    let _ = std::fs::remove_file(&src_path);
    let _ = std::fs::remove_file(&dst_path);
    let _ = std::fs::remove_file(&dump_path);
}

#[tokio::test]
async fn transfer_clean_recreate_sqlite() {
    let src_path = temp_db("xfer-src");
    let dst_path = temp_db("xfer-dst");
    let src = seed(&src_path).await;
    let src_profile = profile(&src_path);
    let dst_profile = profile(&dst_path);
    let dst = providers::connect(&dst_profile).await.expect("dst");

    let report = transfer::transfer(
        &src_profile,
        &dst_profile,
        src.as_ref(),
        dst.as_ref(),
        TransferOptions {
            mode: TransferMode::CleanRecreate,
            assume_yes: true,
            ..Default::default()
        },
        None,
    )
    .await
    .expect("transfer");
    assert!(
        report.passed,
        "src={:?} dst={:?} rows={:?}",
        report.source_counts, report.target_counts, report.row_mismatches
    );

    let _ = std::fs::remove_file(&src_path);
    let _ = std::fs::remove_file(&dst_path);
}
