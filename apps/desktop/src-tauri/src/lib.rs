//! Tauri shell around `mydb-core`. All real logic lives in the core crate;
//! this layer only exposes commands and forwards `TransferEvent`s via Tauri events.

use mydb_core::conn::{ConfigStore, ConnectionProfile, EnvironmentTag, TransferPreset};
use mydb_core::diff::{diff_catalogs, migration_filename};
use mydb_core::dump::{self, detect_tools, DumpOptions, RestoreOptions, ToolBackend};
use mydb_core::introspect::SchemaCatalog;
use mydb_core::provider::DatabaseProvider;
use mydb_core::providers;
use mydb_core::query::{QueryResult, Value};
use mydb_core::sql::sql_literal;
use mydb_core::transfer::{self, TransferEvent, TransferOptions, VerificationReport};
use mydb_core::ProviderKind;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    app_version: String,
    core_version: String,
    config_path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionSummary {
    name: String,
    env: String,
    kind: String,
    target: String,
    ssh: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolStatus {
    backend: String,
    path: String,
    version: String,
    status_line: String,
}

fn env_badge(env: EnvironmentTag) -> &'static str {
    match env {
        EnvironmentTag::Production => "🔴 production",
        EnvironmentTag::Staging => "🟡 staging",
        EnvironmentTag::Development => "🟢 development",
        EnvironmentTag::Testing => "🔵 testing",
        EnvironmentTag::Other => "⚪ other",
    }
}

fn store() -> Result<ConfigStore, String> {
    ConfigStore::load_default().map_err(|e| e.to_string())
}

async fn live(name: &str) -> Result<(ConnectionProfile, Box<dyn DatabaseProvider>), String> {
    let store = store()?;
    let profile = store.resolved(name).map_err(|e| e.to_string())?;
    let provider = providers::connect(&profile)
        .await
        .map_err(|e| e.to_string())?;
    Ok((profile, provider))
}

#[tauri::command]
fn app_info() -> Result<AppInfo, String> {
    let store = store()?;
    Ok(AppInfo {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        core_version: mydb_core::VERSION.to_string(),
        config_path: store.path().display().to_string(),
    })
}

#[tauri::command]
fn list_connections() -> Result<Vec<ConnectionSummary>, String> {
    let store = store()?;
    Ok(store
        .connections()
        .iter()
        .map(|c| ConnectionSummary {
            name: c.name.clone(),
            env: env_badge(c.env).to_string(),
            kind: c.kind.as_str().to_string(),
            target: c.masked_target(),
            ssh: c.ssh.as_ref().map(|s| format!("{}@{}", s.user, s.host)),
        })
        .collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewConnection {
    name: String,
    kind: String,
    env: String,
    url: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    password: Option<String>,
    database: Option<String>,
}

#[tauri::command]
fn add_connection(conn: NewConnection) -> Result<(), String> {
    let mut store = store()?;
    let kind: ProviderKind = conn
        .kind
        .parse()
        .map_err(|e: mydb_core::MyDbError| e.to_string())?;
    let env: EnvironmentTag = conn
        .env
        .parse()
        .map_err(|e: mydb_core::MyDbError| e.to_string())?;
    store
        .add(ConnectionProfile {
            name: conn.name,
            kind,
            env,
            url: conn.url,
            host: conn.host,
            port: conn.port,
            user: conn.user,
            password: conn.password,
            database: conn.database,
            ..Default::default()
        })
        .map_err(|e| e.to_string())?;
    store.save().map_err(|e| e.to_string())
}

#[tauri::command]
fn remove_connection(name: String) -> Result<(), String> {
    let mut store = store()?;
    store.remove(&name).map_err(|e| e.to_string())?;
    store.save().map_err(|e| e.to_string())
}

#[tauri::command]
async fn test_connection(name: String) -> Result<String, String> {
    let (_, provider) = live(&name).await?;
    provider.server_version().await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn introspect(name: String) -> Result<SchemaCatalog, String> {
    let (_, provider) = live(&name).await?;
    provider.introspect().await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn execute_sql(name: String, sql: String) -> Result<QueryResult, String> {
    let (_, provider) = live(&name).await?;
    provider
        .execute_query(&sql)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn fetch_rows(
    name: String,
    schema: String,
    table: String,
    limit: u32,
    offset: u32,
) -> Result<QueryResult, String> {
    let (_, provider) = live(&name).await?;
    let q = provider.kind().qualify(&schema, &table);
    provider
        .execute_query(&format!("SELECT * FROM {q} LIMIT {limit} OFFSET {offset}"))
        .await
        .map_err(|e| e.to_string())
}

#[derive(Deserialize)]
struct CellUpdate {
    column: String,
    value: Value,
}

#[tauri::command]
async fn update_cell(
    name: String,
    schema: String,
    table: String,
    pk: Vec<(String, Value)>,
    update: CellUpdate,
) -> Result<u64, String> {
    let (_, provider) = live(&name).await?;
    let kind = provider.kind();
    let set = format!(
        "{} = {}",
        kind.quote_ident(&update.column),
        sql_literal(kind, &update.value)
    );
    let where_clause: Vec<String> = pk
        .iter()
        .map(|(c, v)| format!("{} = {}", kind.quote_ident(c), sql_literal(kind, v)))
        .collect();
    let sql = format!(
        "UPDATE {} SET {set} WHERE {}",
        kind.qualify(&schema, &table),
        where_clause.join(" AND ")
    );
    let result = provider
        .execute_query(&sql)
        .await
        .map_err(|e| e.to_string())?;
    Ok(result.rows_affected)
}

#[tauri::command]
async fn detect_profile_tools(name: String) -> Result<Vec<ToolStatus>, String> {
    let store = store()?;
    let profile = store.resolved(&name).map_err(|e| e.to_string())?;
    let tools = detect_tools(&profile).await;
    Ok(tools
        .into_iter()
        .map(|t| ToolStatus {
            backend: t.backend.to_string(),
            path: t.path.display().to_string(),
            version: t.version.clone(),
            status_line: t.status_line(),
        })
        .collect())
}

#[tauri::command]
async fn dump_database(name: String, output: String, schema_only: bool) -> Result<String, String> {
    let (profile, provider) = live(&name).await?;
    dump::dump(
        &profile,
        provider.as_ref(),
        DumpOptions {
            output: Some(output.into()),
            schema_only,
            backend: Some(ToolBackend::Native),
            format: mydb_core::dump::DumpFormat::Sql,
            ..Default::default()
        },
        |_| {},
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn restore_database(name: String, file: String, clean: bool) -> Result<usize, String> {
    let (profile, provider) = live(&name).await?;
    dump::restore(
        &profile,
        provider.as_ref(),
        std::path::Path::new(&file),
        RestoreOptions {
            clean,
            if_exists: true,
            ..Default::default()
        },
        |_| {},
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn run_transfer(
    app: AppHandle,
    source: String,
    target: String,
    options: TransferOptions,
) -> Result<VerificationReport, String> {
    let src_profile = store()?.resolved(&source).map_err(|e| e.to_string())?;
    let dst_profile = store()?.resolved(&target).map_err(|e| e.to_string())?;
    let src = providers::connect(&src_profile)
        .await
        .map_err(|e| e.to_string())?;
    let dst = providers::connect(&dst_profile)
        .await
        .map_err(|e| e.to_string())?;
    let (tx, mut rx) = tokio::sync::broadcast::channel::<TransferEvent>(128);
    let app2 = app.clone();
    tokio::spawn(async move {
        while let Ok(ev) = rx.recv().await {
            let _ = app2.emit("transfer-event", ev);
        }
    });
    transfer::transfer(
        &src_profile,
        &dst_profile,
        src.as_ref(),
        dst.as_ref(),
        options,
        Some(tx),
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn preview_plan(
    source: String,
    target: String,
    options: TransferOptions,
) -> Result<transfer::TransferPlan, String> {
    let (_, src) = live(&source).await?;
    let (_, dst) = live(&target).await?;
    let src_cat = src.introspect().await.map_err(|e| e.to_string())?;
    let dst_cat = dst.introspect().await.map_err(|e| e.to_string())?;
    Ok(transfer::build_plan(&src_cat, &dst_cat, &options))
}

#[tauri::command]
async fn run_diff(source: String, target: String) -> Result<mydb_core::diff::SchemaDiff, String> {
    let (_, src) = live(&source).await?;
    let (_, dst) = live(&target).await?;
    let src_cat = src.introspect().await.map_err(|e| e.to_string())?;
    let dst_cat = dst.introspect().await.map_err(|e| e.to_string())?;
    Ok(diff_catalogs(&src_cat, &dst_cat))
}

#[tauri::command]
async fn save_migration(source: String, target: String, dir: String) -> Result<String, String> {
    let diff = run_diff(source.clone(), target.clone()).await?;
    let mut script = String::from("-- generated by MyDB Manager; LF line endings\n");
    script.push_str(&diff.sql());
    if !script.ends_with('\n') {
        script.push('\n');
    }
    let path = std::path::Path::new(&dir).join(migration_filename(&source, &target));
    tokio::fs::write(&path, script.replace('\r', ""))
        .await
        .map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}

#[tauri::command]
async fn run_verify(source: String, target: String) -> Result<VerificationReport, String> {
    let (_, src) = live(&source).await?;
    let (_, dst) = live(&target).await?;
    dump::verify_providers(src.as_ref(), dst.as_ref())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn list_presets() -> Result<Vec<TransferPreset>, String> {
    Ok(store()?.presets().to_vec())
}

#[tauri::command]
fn save_preset(preset: TransferPreset) -> Result<(), String> {
    let mut store = store()?;
    store.upsert_preset(preset).map_err(|e| e.to_string())?;
    store.save().map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            app_info,
            list_connections,
            add_connection,
            remove_connection,
            test_connection,
            introspect,
            execute_sql,
            fetch_rows,
            update_cell,
            detect_profile_tools,
            dump_database,
            restore_database,
            run_transfer,
            preview_plan,
            run_diff,
            save_migration,
            run_verify,
            list_presets,
            save_preset,
        ])
        .run(tauri::generate_context!())
        .expect("error while running MyDB Manager");
}

#[cfg(test)]
mod tests {
    #[test]
    fn smoke_version_present() {
        assert!(!env!("CARGO_PKG_VERSION").is_empty());
    }
}
