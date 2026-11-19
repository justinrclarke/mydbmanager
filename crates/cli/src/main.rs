//! `mydb` — the command-line companion to the MyDB Manager desktop app.
//!
//! Connection management works today; introspection/query land in Phase 1,
//! dump/restore in Phase 2, transfer/diff/verify in Phase 3.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};
use comfy_table::presets::UTF8_NO_BORDERS;
use comfy_table::{Attribute, Cell, ContentArrangement, Table};
use mydb_core::conn::{ConfigStore, ConnectionProfile, EnvironmentTag};
use mydb_core::dump::DumpFormat;
use mydb_core::transfer::TransferMode;
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
    /// List database objects (tables, views, sequences, ...). [Phase 1]
    Ls {
        /// Connection profile name.
        conn: String,
        /// Restrict to one schema.
        #[arg(long)]
        schema: Option<String>,
    },
    /// Run a SQL query against a connection. [Phase 1]
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
    /// Test connectivity to a profile. [Phase 1]
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

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::filter::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::filter::EnvFilter::new("warn")),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();
    if let Err(err) = run(cli) {
        eprintln!("error: {err:#}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn run(cli: Cli) -> anyhow::Result<()> {
    let store = match &cli.config {
        Some(path) => ConfigStore::open(path.clone())?,
        None => ConfigStore::load_default()?,
    };

    match cli.command {
        Command::Conn { command } => conn_command(command, store),
        Command::Ls { .. } => not_yet("mydb ls", "Phase 1 (introspection)"),
        Command::Query { .. } => not_yet("mydb query", "Phase 1 (query execution)"),
        Command::Dump { format, .. } => {
            // Validate user input even for stubbed commands, so scripts fail fast.
            let _format: DumpFormat = format
                .parse()
                .map_err(|e: mydb_core::MyDbError| anyhow::anyhow!("{e}"))?;
            not_yet("mydb dump", "Phase 2 (dump/restore engine)")
        }
        Command::Restore { .. } => not_yet("mydb restore", "Phase 2 (dump/restore engine)"),
        Command::Transfer { mode, .. } => {
            let _mode: TransferMode = mode
                .parse()
                .map_err(|e: mydb_core::MyDbError| anyhow::anyhow!("{e}"))?;
            not_yet("mydb transfer", "Phase 3 (transfer engine)")
        }
        Command::Diff { .. } => not_yet("mydb diff", "Phase 3 (diff engine)"),
        Command::Verify { .. } => not_yet("mydb verify", "Phase 3 (verification)"),
    }
}

fn not_yet(command: &str, phase: &str) -> anyhow::Result<()> {
    Err(anyhow::anyhow!(
        "`{command}` arrives in {phase} — the core engine is scaffolded but not wired up yet"
    ))
}

fn conn_command(command: ConnCommand, mut store: ConfigStore) -> anyhow::Result<()> {
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
            // Make sure the profile exists now; actual connectivity lands in Phase 1.
            store.get(&name)?;
            not_yet("mydb conn test", "Phase 1 (connection pooling)")
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
