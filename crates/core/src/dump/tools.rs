//! Vendor-tool dump/restore (`pg_dump` / `pg_restore` / `mysqldump` / `mysql`).

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;

use crate::conn::ConnectionProfile;
use crate::dump::{DumpFormat, DumpOptions, RestoreOptions, ToolBackend};
use crate::error::{MyDbError, Result};
use crate::provider::ProviderKind;
use crate::providers::build_url;
use crate::sql::find_executable;
use crate::ssh::destination;

/// A detected (or overridden) client tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedTool {
    /// Which backend this is.
    pub backend: ToolBackend,
    /// Absolute path.
    pub path: PathBuf,
    /// `pg_dump (PostgreSQL) 16.4` style version string.
    pub version: String,
}

impl DetectedTool {
    /// Short status line for CLI/UI.
    pub fn status_line(&self) -> String {
        format!("{} found ✓ — {}", self.path.display(), self.version.trim())
    }
}

/// Probe PATH and per-profile overrides for dump/restore tools.
pub async fn detect(profile: &ConnectionProfile) -> Vec<DetectedTool> {
    let mut found = Vec::new();
    match profile.kind {
        ProviderKind::Postgres => {
            if let Some(tool) =
                probe(profile.pg_dump_path.clone(), "pg_dump", ToolBackend::PgDump).await
            {
                found.push(tool);
            }
            if let Some(tool) = probe(
                profile.pg_restore_path.clone(),
                "pg_restore",
                ToolBackend::PgRestore,
            )
            .await
            {
                found.push(tool);
            }
        }
        ProviderKind::Mysql => {
            if let Some(tool) = probe(
                profile.mysqldump_path.clone(),
                "mysqldump",
                ToolBackend::MySqlDump,
            )
            .await
            {
                found.push(tool);
            }
            if let Some(tool) = probe(None, "mysql", ToolBackend::MySql).await {
                found.push(tool);
            }
        }
        ProviderKind::Sqlite => {}
    }
    found
}

async fn probe(
    override_path: Option<PathBuf>,
    name: &str,
    backend: ToolBackend,
) -> Option<DetectedTool> {
    let path = override_path.or_else(|| find_executable(name))?;
    let output = Command::new(&path).arg("--version").output().await.ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Some(DetectedTool {
        backend,
        path,
        version,
    })
}

/// Pick a dump backend: explicit option, else vendor tool, else native.
pub async fn choose_dump_backend(
    profile: &ConnectionProfile,
    options: &DumpOptions,
) -> ToolBackend {
    if let Some(backend) = options.backend {
        return backend;
    }
    let tools = detect(profile).await;
    match profile.kind {
        ProviderKind::Postgres if tools.iter().any(|t| t.backend == ToolBackend::PgDump) => {
            ToolBackend::PgDump
        }
        ProviderKind::Mysql if tools.iter().any(|t| t.backend == ToolBackend::MySqlDump) => {
            ToolBackend::MySqlDump
        }
        _ => ToolBackend::Native,
    }
}

/// Run `pg_dump` or `mysqldump` into `output`.
pub async fn dump_with_tool(
    profile: &ConnectionProfile,
    options: &DumpOptions,
    backend: ToolBackend,
    output: &Path,
) -> Result<()> {
    let url = build_url(profile)?;
    match backend {
        ToolBackend::PgDump => pg_dump(profile, options, &url, output).await,
        ToolBackend::MySqlDump => mysqldump(profile, options, output).await,
        _ => Err(MyDbError::Tool(format!("{backend:?} is not a dump tool"))),
    }
}

async fn pg_dump(
    profile: &ConnectionProfile,
    options: &DumpOptions,
    url: &str,
    output: &Path,
) -> Result<()> {
    let tool = probe(profile.pg_dump_path.clone(), "pg_dump", ToolBackend::PgDump)
        .await
        .ok_or_else(|| MyDbError::Tool("pg_dump not found on PATH".into()))?;
    let mut cmd = Command::new(&tool.path);
    cmd.arg("--no-owner").arg("--no-acl");
    match options.format {
        DumpFormat::Custom => {
            cmd.arg("-Fc");
        }
        DumpFormat::Directory => {
            cmd.arg("-Fd");
        }
        DumpFormat::Sql => {
            cmd.arg("-Fp");
        }
    }
    if options.schema_only {
        cmd.arg("--schema-only");
    }
    if options.data_only {
        cmd.arg("--data-only");
    }
    if options.clean {
        cmd.arg("--clean").arg("--if-exists");
    }
    for schema in &options.schemas {
        cmd.arg("-n").arg(schema);
    }
    for table in &options.tables {
        cmd.arg("-t").arg(table);
    }
    for table in &options.exclude_tables {
        cmd.arg("--exclude-table").arg(table);
    }
    cmd.arg("-f").arg(output);
    cmd.arg(url);
    cmd.stdin(Stdio::null());
    run(cmd, "pg_dump").await
}

async fn mysqldump(
    profile: &ConnectionProfile,
    options: &DumpOptions,
    output: &Path,
) -> Result<()> {
    let tool = probe(
        profile.mysqldump_path.clone(),
        "mysqldump",
        ToolBackend::MySqlDump,
    )
    .await
    .ok_or_else(|| MyDbError::Tool("mysqldump not found on PATH".into()))?;
    let (host, port) = destination(profile)?;
    let mut cmd = Command::new(&tool.path);
    cmd.arg("--single-transaction")
        .arg("--routines")
        .arg("--triggers")
        .arg("--events")
        .arg("--set-gtid-purged=OFF")
        .arg("-h")
        .arg(&host)
        .arg("-P")
        .arg(port.to_string());
    if let Some(user) = &profile.user {
        cmd.arg("-u").arg(user);
    }
    if let Some(password) = &profile.password {
        cmd.env("MYSQL_PWD", password);
    }
    if options.schema_only {
        cmd.arg("--no-data");
    }
    if options.data_only {
        cmd.arg("--no-create-info");
    }
    if let Some(db) = &profile.database {
        cmd.arg(db);
    }
    for table in &options.exclude_tables {
        cmd.arg("--ignore-table").arg(table);
    }
    cmd.arg("-r").arg(output);
    cmd.stdin(Stdio::null());
    run(cmd, "mysqldump").await
}

/// Restore a vendor dump into `profile`.
pub async fn restore_with_tool(
    profile: &ConnectionProfile,
    input: &Path,
    options: &RestoreOptions,
    backend: ToolBackend,
) -> Result<()> {
    if options.dry_run {
        return Ok(());
    }
    match backend {
        ToolBackend::PgRestore => pg_restore(profile, input, options).await,
        ToolBackend::MySql => mysql_restore(profile, input).await,
        _ => Err(MyDbError::Tool(format!(
            "{backend:?} is not a restore tool"
        ))),
    }
}

async fn pg_restore(
    profile: &ConnectionProfile,
    input: &Path,
    options: &RestoreOptions,
) -> Result<()> {
    let tool = probe(
        profile.pg_restore_path.clone(),
        "pg_restore",
        ToolBackend::PgRestore,
    )
    .await
    .ok_or_else(|| MyDbError::Tool("pg_restore not found on PATH".into()))?;
    let url = build_url(profile)?;
    let mut cmd = Command::new(&tool.path);
    cmd.arg("--no-owner").arg("--no-acl");
    if options.clean {
        cmd.arg("--clean");
    }
    if options.if_exists {
        cmd.arg("--if-exists");
    }
    if let Some(jobs) = options.jobs {
        cmd.arg("-j").arg(jobs.to_string());
    }
    cmd.arg("-d").arg(url);
    cmd.arg(input);
    cmd.stdin(Stdio::null());
    run(cmd, "pg_restore").await
}

async fn mysql_restore(profile: &ConnectionProfile, input: &Path) -> Result<()> {
    let tool = probe(None, "mysql", ToolBackend::MySql)
        .await
        .ok_or_else(|| MyDbError::Tool("mysql client not found on PATH".into()))?;
    let (host, port) = destination(profile)?;
    let mut cmd = Command::new(&tool.path);
    cmd.arg("-h").arg(&host).arg("-P").arg(port.to_string());
    if let Some(user) = &profile.user {
        cmd.arg("-u").arg(user);
    }
    if let Some(password) = &profile.password {
        cmd.env("MYSQL_PWD", password);
    }
    if let Some(db) = &profile.database {
        cmd.arg(db);
    }
    let sql = tokio::fs::read(input).await?;
    cmd.stdin(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| MyDbError::Tool(format!("cannot spawn mysql: {e}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin
            .write_all(&sql)
            .await
            .map_err(|e| MyDbError::Tool(format!("cannot write to mysql: {e}")))?;
    }
    let status = child
        .wait()
        .await
        .map_err(|e| MyDbError::Tool(format!("mysql wait: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(MyDbError::Tool(format!("mysql exited with {status}")))
    }
}

/// Guess restore backend from the dump path.
pub fn restore_backend_for(path: &Path, kind: ProviderKind) -> ToolBackend {
    if path.is_dir() {
        return match kind {
            ProviderKind::Postgres => ToolBackend::PgRestore,
            _ => ToolBackend::Native,
        };
    }
    if looks_like_pg_custom(path) {
        return ToolBackend::PgRestore;
    }
    match kind {
        ProviderKind::Mysql => ToolBackend::MySql,
        _ => ToolBackend::Native,
    }
}

fn looks_like_pg_custom(path: &Path) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = [0u8; 5];
    use std::io::Read;
    matches!(f.read(&mut buf), Ok(n) if n >= 5 && &buf == b"PGDMP")
}

async fn run(mut cmd: Command, name: &str) -> Result<()> {
    let output = cmd
        .output()
        .await
        .map_err(|e| MyDbError::Tool(format!("cannot spawn {name}: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(MyDbError::Tool(format!(
            "{name} failed ({:?}): {}",
            output.status.code(),
            stderr.trim()
        )))
    }
}
