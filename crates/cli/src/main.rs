//! `mydb` — the command-line companion to the MyDB Manager desktop app.

mod render;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};
use comfy_table::presets::UTF8_NO_BORDERS;
use comfy_table::{Attribute, Cell, ContentArrangement, Table};
use mydb_core::conn::{ConfigStore, ConnectionProfile, EnvironmentTag, TransferPreset};
use mydb_core::diff::{self, diff_catalogs};
use mydb_core::dump::{self, DumpOptions, RestoreOptions, ToolBackend};
use mydb_core::provider::DatabaseProvider;
use mydb_core::providers;
use mydb_core::transfer::{self, TransferEvent, TransferOptions};
use mydb_core::ProviderKind;

#[derive(Parser)]
#[command(
    name = "mydb",
    version,
    about = "Manage, dump, restore and transfer PostgreSQL, MySQL and SQLite databases",
    long_about = None
)]
struct Cli {
    /// Path to an alternate connections.toml config file.
    #[arg(long, global = true, env = "MYDB_CONFIG")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Manage connection profiles (shared with the desktop app).
    Conn {
        #[command(subcommand)]
        command: ConnCommand,
    },
    /// List database objects (tables, views, sequences, ...).
    Ls {
        /// Connection profile name.
        conn: String,
        /// Restrict to one schema.
        #[arg(long)]
        schema: Option<String>,
    },
    /// Run a SQL query against a connection.
    Query {
        /// Connection profile name.
        conn: String,
        /// SQL statement to execute.
        sql: String,
        /// Output format.
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,
    },
    /// Dump a database to a file. [Phase 2]
    Dump {
        /// Connection profile name.
        conn: String,
        /// Output file (or directory for --format directory); defaults to stdout for sql format.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Dump format.
        #[arg(long, default_value = "custom")]
        format: String,
        /// Dump schema only, no data.
        #[arg(long)]
        schema_only: bool,
        /// Include DROP statements for a clean restore.
        #[arg(long)]
        clean: bool,
        /// Tables to exclude (repeatable, globs allowed).
        #[arg(long)]
        exclude_table: Vec<String>,
        /// Force a backend (`native`, `pg_dump`, `mysqldump`).
        #[arg(long)]
        backend: Option<String>,
    },
    /// Restore a dump into a database. [Phase 2]
    Restore {
        /// Dump file (or directory) to restore.
        file: PathBuf,
        /// Target connection profile name.
        #[arg(long)]
        into: String,
        /// Drop existing objects before recreating.
        #[arg(long)]
        clean: bool,
        /// Show what would happen without executing.
        #[arg(long)]
        dry_run: bool,
        /// Skip post-restore verification.
        #[arg(long)]
        skip_verify: bool,
    },
    /// Transfer a database from one environment to another. [Phase 3]
    Transfer {
        /// Source connection profile name.
        source: String,
        /// Target connection profile name.
        target: String,
        /// How the target is handled.
        #[arg(long, default_value = "clean-recreate")]
        mode: String,
        /// Tables to exclude (repeatable, globs allowed).
        #[arg(long)]
        exclude_table: Vec<String>,
        /// Copy schema but no data.
        #[arg(long)]
        schema_only: bool,
        /// Build and print the plan without touching the target.
        #[arg(long)]
        dry_run: bool,
        /// Skip interactive confirmation (required for production targets).
        #[arg(short = 'y', long)]
        assume_yes: bool,
        /// Saved preset name.
        #[arg(long)]
        preset: Option<String>,
        /// Skip verification.
        #[arg(long)]
        skip_verify: bool,
        /// Save this transfer as a named preset.
        #[arg(long)]
        save_preset: Option<String>,
    },
    /// Show schema differences between two databases. [Phase 3]
    Diff {
        /// Source connection profile name.
        source: String,
        /// Target connection profile name.
        target: String,
        /// Print migration SQL instead of a change list.
        #[arg(long)]
        sql: bool,
        /// Write SQL to this path (timestamped name if a directory).
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Verify that two databases match (objects, rows, sequences). [Phase 3]
    Verify {
        /// Source connection profile name.
        source: String,
        /// Target connection profile name.
        target: String,
    },
}

#[derive(Subcommand)]
enum ConnCommand {
    /// Add a new connection profile.
    Add(#[command(flatten)] Box<ConnAddArgs>),
    /// List saved connection profiles.
    List,
    /// Test connectivity to a profile.
    Test {
        /// Connection profile name.
        name: String,
    },
    /// Remove a connection profile.
    Rm {
        /// Connection profile name.
        name: String,
    },
}

#[derive(clap::Args)]
struct ConnAddArgs {
    /// Unique profile name (used everywhere on the command line).
    name: String,
    /// Full connection URL, e.g. postgres://user:pass@host:5432/db.
    #[arg(long, conflicts_with_all = ["host", "port", "user", "password", "database"])]
    url: Option<String>,
    /// Hostname.
    #[arg(long)]
    host: Option<String>,
    /// TCP port (defaults to the engine's conventional port).
    #[arg(long)]
    port: Option<u16>,
    /// User name.
    #[arg(long)]
    user: Option<String>,
    /// Password (stored in config for now; OS keyring in Phase 1).
    #[arg(long, env = "MYDB_PASSWORD")]
    password: Option<String>,
    /// Database name, or file path for SQLite.
    #[arg(long)]
    database: Option<String>,
    /// Environment label.
    #[arg(long, value_enum, default_value = "dev")]
    env: EnvArg,
    /// Database engine (optional when --url is given; inferred from the URL scheme).
    #[arg(long, value_enum)]
    kind: Option<KindArg>,
    /// SSH bastion host for tunneling, e.g. bastion.example.com.
    #[arg(long, requires = "ssh_user")]
    ssh_host: Option<String>,
    /// SSH port (default 22).
    #[arg(long, requires = "ssh_host")]
    ssh_port: Option<u16>,
    /// SSH user.
    #[arg(long, requires = "ssh_host")]
    ssh_user: Option<String>,
    /// Path to an SSH private key.
    #[arg(long, requires = "ssh_host")]
    ssh_key: Option<PathBuf>,
    /// Override path to pg_dump (Postgres only).
    #[arg(long)]
    pg_dump_path: Option<PathBuf>,
    /// Override path to mysqldump (MySQL only).
    #[arg(long)]
    mysqldump_path: Option<PathBuf>,
}

#[derive(Clone, Copy, ValueEnum)]
enum EnvArg {
    Prod,
    Staging,
    Dev,
    Test,
    Other,
}

impl From<EnvArg> for EnvironmentTag {
    fn from(value: EnvArg) -> Self {
        match value {
            EnvArg::Prod => EnvironmentTag::Production,
            EnvArg::Staging => EnvironmentTag::Staging,
            EnvArg::Dev => EnvironmentTag::Development,
            EnvArg::Test => EnvironmentTag::Testing,
            EnvArg::Other => EnvironmentTag::Other,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum KindArg {
    Postgres,
    Mysql,
    Sqlite,
}

impl From<KindArg> for ProviderKind {
    fn from(value: KindArg) -> Self {
        match value {
            KindArg::Postgres => ProviderKind::Postgres,
            KindArg::Mysql => ProviderKind::Mysql,
            KindArg::Sqlite => ProviderKind::Sqlite,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum OutputFormat {
    Table,
    Csv,
    Json,
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::filter::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::filter::EnvFilter::new("warn")),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();
    if let Err(err) = run(cli).await {
        eprintln!("error: {err:#}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    let mut store = match &cli.config {
        Some(path) => ConfigStore::open(path.clone())?,
        None => ConfigStore::load_default()?,
    };

    match cli.command {
        Command::Conn { command } => conn_command(command, &mut store).await,
        Command::Ls { conn, schema } => {
            let provider = open_provider(&store, &conn).await?;
            let catalog = provider
                .introspect()
                .await
                .with_context(|| format!("cannot introspect '{conn}'"))?;
            print!("{}", render::catalog(&catalog, schema.as_deref()));
            Ok(())
        }
        Command::Query { conn, sql, format } => {
            let provider = open_provider(&store, &conn).await?;
            let result = provider
                .execute_query(&sql)
                .await
                .with_context(|| format!("query failed on '{conn}'"))?;
            match format {
                OutputFormat::Table => print!("{}", render::query_table(&result)),
                OutputFormat::Csv => print!("{}", render::query_csv(&result)),
                OutputFormat::Json => println!("{}", render::query_json(&result)),
            }
            Ok(())
        }
        Command::Dump {
            conn,
            output,
            format,
            schema_only,
            clean,
            exclude_table,
            backend,
        } => {
            let profile = store.resolved(&conn)?;
            let provider = open_provider(&store, &conn).await?;
            let options = DumpOptions {
                format: format
                    .parse()
                    .map_err(|e: mydb_core::MyDbError| anyhow::anyhow!("{e}"))?,
                output: output.clone(),
                schema_only,
                clean,
                exclude_tables: exclude_table,
                backend: backend
                    .as_deref()
                    .map(|b| b.parse::<ToolBackend>())
                    .transpose()
                    .map_err(|e: mydb_core::MyDbError| anyhow::anyhow!("{e}"))?,
                ..Default::default()
            };
            let tools = dump::detect_tools(&profile).await;
            if tools.is_empty() {
                eprintln!("tools: none found — using built-in engine");
            } else {
                for t in &tools {
                    eprintln!("tools: {}", t.status_line());
                }
            }
            let to_stdout = options.output.is_none();
            let body = dump::dump(&profile, provider.as_ref(), options, |ev| {
                eprint_event(&ev);
            })
            .await?;
            if to_stdout {
                print!("{body}");
            } else {
                println!("wrote {body}");
            }
            Ok(())
        }
        Command::Restore {
            file,
            into,
            clean,
            dry_run,
            skip_verify,
        } => {
            let profile = store.resolved(&into)?;
            let provider = open_provider(&store, &into).await?;
            dump::restore(
                &profile,
                provider.as_ref(),
                &file,
                RestoreOptions {
                    clean,
                    if_exists: true,
                    no_owner: true,
                    dry_run,
                    skip_verify,
                    ..Default::default()
                },
                |ev| eprint_event(&ev),
            )
            .await?;
            Ok(())
        }
        Command::Transfer {
            source,
            target,
            mode,
            exclude_table,
            schema_only,
            dry_run,
            assume_yes,
            preset,
            skip_verify,
            save_preset,
        } => {
            let (source, target, mut options) = if let Some(name) = preset {
                let p = store
                    .presets()
                    .iter()
                    .find(|p| p.name == name)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("preset '{name}' not found"))?;
                (
                    p.source,
                    p.target,
                    TransferOptions {
                        mode: p.mode,
                        exclude_tables: p.exclude_tables,
                        schema_only: p.schema_only,
                        dry_run,
                        assume_yes,
                        skip_verify,
                        ..Default::default()
                    },
                )
            } else {
                (
                    source,
                    target,
                    TransferOptions {
                        mode: mode
                            .parse()
                            .map_err(|e: mydb_core::MyDbError| anyhow::anyhow!("{e}"))?,
                        exclude_tables: exclude_table,
                        schema_only,
                        dry_run,
                        assume_yes,
                        skip_verify,
                        ..Default::default()
                    },
                )
            };
            if let Some(name) = save_preset {
                store.upsert_preset(TransferPreset {
                    name,
                    source: source.clone(),
                    target: target.clone(),
                    mode: options.mode,
                    exclude_tables: options.exclude_tables.clone(),
                    schema_only: options.schema_only,
                })?;
                store.save()?;
            }
            let src_profile = store.resolved(&source)?;
            let dst_profile = store.resolved(&target)?;
            let src = providers::connect(&src_profile).await?;
            let dst = providers::connect(&dst_profile).await?;
            if dry_run {
                let src_cat = src.introspect().await?;
                let dst_cat = dst.introspect().await?;
                let plan = transfer::build_plan(&src_cat, &dst_cat, &options);
                print!("{}", transfer::format_plan(&plan));
                return Ok(());
            }
            let (tx, mut rx) = tokio::sync::broadcast::channel::<TransferEvent>(64);
            let progress = tokio::spawn(async move {
                while let Ok(ev) = rx.recv().await {
                    eprint_event(&ev);
                }
            });
            options.dry_run = dry_run;
            let report = transfer::transfer(
                &src_profile,
                &dst_profile,
                src.as_ref(),
                dst.as_ref(),
                options,
                Some(tx),
            )
            .await?;
            drop(progress);
            print_report(&report);
            if !report.passed {
                anyhow::bail!("verification failed");
            }
            Ok(())
        }
        Command::Diff {
            source,
            target,
            sql,
            output,
        } => {
            let src = open_provider(&store, &source).await?;
            let dst = open_provider(&store, &target).await?;
            let src_cat = src.introspect().await?;
            let dst_cat = dst.introspect().await?;
            let diff = diff_catalogs(&src_cat, &dst_cat);
            if diff.is_empty() {
                println!("no differences");
                return Ok(());
            }
            let body = if sql {
                let mut script = String::from("-- generated by mydb diff; LF line endings\n");
                script.push_str(&diff.sql());
                if !script.ends_with('\n') {
                    script.push('\n');
                }
                script
            } else {
                let mut out = String::new();
                for item in &diff.items {
                    out.push_str(&format!(
                        "{:?} {} — {}\n",
                        item.action,
                        render::object_ref(item.kind, &item.schema, &item.name),
                        item.detail
                    ));
                }
                out
            };
            if let Some(path) = output {
                let path = if path.is_dir() {
                    path.join(diff::migration_filename(&source, &target))
                } else {
                    path
                };
                tokio::fs::write(&path, body.replace('\r', "").as_bytes()).await?;
                println!("wrote {}", path.display());
            } else {
                print!("{body}");
            }
            Ok(())
        }
        Command::Verify { source, target } => {
            let src = open_provider(&store, &source).await?;
            let dst = open_provider(&store, &target).await?;
            let report = dump::verify_providers(src.as_ref(), dst.as_ref()).await?;
            print_report(&report);
            if !report.passed {
                anyhow::bail!("verification failed");
            }
            Ok(())
        }
    }
}

fn eprint_event(ev: &TransferEvent) {
    if let (Some(table), Some(done), Some(total)) = (&ev.table, ev.rows_done, ev.rows_total) {
        eprintln!("[{}] {table}: {done}/{total}", ev.kind.as_str());
    } else {
        eprintln!("[{}] {}", ev.kind.as_str(), ev.message);
    }
}

fn print_report(report: &mydb_core::transfer::VerificationReport) {
    println!(
        "verification: {}",
        if report.passed { "passed" } else { "FAILED" }
    );
    if !report.row_mismatches.is_empty() {
        println!("row mismatches:");
        for (t, s, d) in &report.row_mismatches {
            println!("  {t}: source={s} target={d}");
        }
    }
    if !report.sequence_mismatches.is_empty() {
        println!("sequence mismatches:");
        for (n, s, d) in &report.sequence_mismatches {
            println!("  {n}: source={s:?} target={d:?}");
        }
    }
}

/// Resolve a profile by name and return a connected provider.
async fn open_provider(
    store: &ConfigStore,
    name: &str,
) -> anyhow::Result<Box<dyn DatabaseProvider>> {
    let profile = store.resolved(name)?;
    providers::connect(&profile)
        .await
        .with_context(|| format!("cannot connect to '{name}' ({})", profile.masked_target()))
}

async fn conn_command(command: ConnCommand, store: &mut ConfigStore) -> anyhow::Result<()> {
    match command {
        ConnCommand::Add(args) => {
            let ConnAddArgs {
                name,
                url,
                host,
                port,
                user,
                password,
                database,
                env,
                kind,
                ssh_host,
                ssh_port,
                ssh_user,
                ssh_key,
                pg_dump_path,
                mysqldump_path,
            } = *args;
            let env: EnvironmentTag = env.into();
            let kind = match kind {
                Some(kind) => kind.into(),
                None => url
                    .as_deref()
                    .map(kind_from_url)
                    .transpose()?
                    .ok_or_else(|| anyhow::anyhow!("--kind is required when no --url is given"))?,
            };
            let ssh = match (&ssh_host, &ssh_user) {
                (Some(host), Some(user)) => Some(mydb_core::conn::SshConfig {
                    host: host.clone(),
                    port: ssh_port.unwrap_or(22),
                    user: user.clone(),
                    password: None,
                    private_key_path: ssh_key.clone(),
                    passphrase: None,
                }),
                _ => None,
            };
            let profile = ConnectionProfile {
                name: name.clone(),
                env,
                kind,
                url,
                host,
                port,
                user,
                password,
                database,
                ssh,
                pg_dump_path,
                mysqldump_path,
                ..Default::default()
            };
            store.add(profile).context("cannot add connection")?;
            store.save()?;
            println!(
                "added connection '{name}' ({kind}, {env}) to {}",
                store.path().display()
            );
            Ok(())
        }
        ConnCommand::List => {
            let connections = store.connections();
            if connections.is_empty() {
                println!("no connections yet — add one with `mydb conn add <name> --url ...`");
                println!("(config file: {})", store.path().display());
                return Ok(());
            }
            let mut table = Table::new();
            table.load_preset(UTF8_NO_BORDERS);
            table.set_content_arrangement(ContentArrangement::Dynamic);
            table.set_header(vec![
                Cell::new("NAME").add_attribute(Attribute::Bold),
                Cell::new("ENV").add_attribute(Attribute::Bold),
                Cell::new("KIND").add_attribute(Attribute::Bold),
                Cell::new("TARGET").add_attribute(Attribute::Bold),
                Cell::new("SSH").add_attribute(Attribute::Bold),
            ]);
            for conn in connections {
                table.add_row(vec![
                    Cell::new(&conn.name),
                    Cell::new(env_badge(conn.env)),
                    Cell::new(conn.kind.as_str()),
                    Cell::new(conn.masked_target()),
                    Cell::new(
                        conn.ssh
                            .as_ref()
                            .map(|s| format!("{}@{}", s.user, s.host))
                            .unwrap_or_else(|| "-".into()),
                    ),
                ]);
            }
            println!("{table}");
            Ok(())
        }
        ConnCommand::Test { name } => {
            let provider = open_provider(store, &name).await?;
            let version = provider.server_version().await?;
            println!("✓ '{name}' connected — {version}");
            Ok(())
        }
        ConnCommand::Rm { name } => {
            store.remove(&name)?;
            store.save()?;
            println!("removed connection '{name}'");
            Ok(())
        }
    }
}

fn kind_from_url(url: &str) -> anyhow::Result<ProviderKind> {
    let scheme = url
        .split_once("://")
        .map(|(scheme, _)| scheme)
        .ok_or_else(|| {
            anyhow::anyhow!("cannot determine database kind from URL '{url}' (missing scheme)")
        })?;
    scheme
        .parse::<ProviderKind>()
        .map_err(|e| anyhow::anyhow!("{e}"))
}

fn env_badge(env: EnvironmentTag) -> String {
    match env {
        EnvironmentTag::Production => "🔴 production",
        EnvironmentTag::Staging => "🟡 staging",
        EnvironmentTag::Development => "🟢 development",
        EnvironmentTag::Testing => "🔵 testing",
        EnvironmentTag::Other => "⚪ other",
    }
    .to_string()
}
