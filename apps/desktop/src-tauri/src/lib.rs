//! Tauri shell around `mydb-core`. All real logic lives in the core crate;
//! this layer only exposes commands and (from Phase 1 on) forwards
//! `TransferEvent`s to the frontend via Tauri events.

use mydb_core::conn::{ConfigStore, EnvironmentTag};
use serde::Serialize;

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

fn env_badge(env: EnvironmentTag) -> &'static str {
    match env {
        EnvironmentTag::Production => "🔴 production",
        EnvironmentTag::Staging => "🟡 staging",
        EnvironmentTag::Development => "🟢 development",
        EnvironmentTag::Testing => "🔵 testing",
        EnvironmentTag::Other => "⚪ other",
    }
}

#[tauri::command]
fn app_info() -> Result<AppInfo, String> {
    let store = ConfigStore::load_default().map_err(|e| e.to_string())?;
    Ok(AppInfo {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        core_version: mydb_core::VERSION.to_string(),
        config_path: store.path().display().to_string(),
    })
}

#[tauri::command]
fn list_connections() -> Result<Vec<ConnectionSummary>, String> {
    let store = ConfigStore::load_default().map_err(|e| e.to_string())?;
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![app_info, list_connections])
        .run(tauri::generate_context!())
        .expect("error while running MyDB Manager");
}
