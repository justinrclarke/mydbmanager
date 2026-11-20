# MyDB Manager — Project Plan

> Living document. Update when decisions change. Progress tracking lives in
> [MILESTONES.md](./MILESTONES.md); contributor/AI-agent context lives in
> [../AGENTS.md](../AGENTS.md).

## Goals

1. **Move databases between environments (production → staging → dev) without
   losing anything.** Tables, sequences *including their current values*,
   views, materialized views, functions, triggers, indexes, constraints,
   enums/types and extensions are all captured and replayed in dependency
   order, then verified.
2. **Two frontends, one engine.** A Tauri desktop app and a terminal CLI
   (`mydb`) share the exact same Rust core — no duplicated logic, no
   feature drift.
3. **Intuitive UI/UX for full database management**: browse and edit data,
   run SQL, inspect every object type, and drive transfers through a
   plan-review → execute → verify wizard.
4. **Cross-platform**: macOS, Linux, and Windows are first-class everywhere
   (engine, CLI, desktop app, CI, release artifacts).
5. **Supported engines**: PostgreSQL, MySQL/MariaDB, SQLite — behind one
   provider trait with explicit capability flags.

## The core problem this project solves

TablePlus-style dump/import flows silently drop or corrupt things:
sequences go missing, `last_value` resets to 1 (fresh inserts collide with
restored rows), objects get created in the wrong dependency order. MyDB's
answer is a **complete typed model of the database** (`SchemaCatalog`) that
every feature is driven from:

1. Introspect the *full* catalog from the source (including sequence
   `last_value` and ownership).
2. Emit DDL in dependency order: types/enums → sequences → tables →
   constraints → indexes → views → functions → triggers.
3. Copy data in FK-safe order, then `setval()` every sequence.
4. **Verify**: re-introspect the target, compare object counts per type,
   per-table row counts, and sequence values; produce a report.

## Architecture

```
mydbmanager/
├── crates/
│   ├── core/            # mydb-core: all database logic, zero UI deps
│   │   ├── conn/            # shared connection profiles (TOML, platform config dir)
│   │   ├── introspect/      # SchemaCatalog: full typed object graph
│   │   ├── provider.rs      # DatabaseProvider trait + per-engine Capabilities
│   │   ├── providers/       # postgres.rs, mysql.rs, sqlite.rs implementations
│   │   ├── query/           # ad-hoc query results (driver-agnostic Value type)
│   │   ├── dump/            # hybrid dump/restore engine
│   │   ├── transfer/        # env → env transfer + verification reports
│   │   └── diff/            # schema diff + migration SQL
│   └── cli/             # `mydb` binary (clap)
└── apps/
    └── desktop/         # Tauri v2 app
        ├── src-tauri/       # thin Rust shell over mydb-core
        └── src/             # Svelte 5 + TypeScript + Vite frontend
```

### Key technical decisions

| Decision | Choice | Rationale |
|---|---|---|
| Dump/restore strategy | **Hybrid** | Use `pg_dump`/`pg_restore`/`mysqldump` when detected (battle-tested fidelity, per-connection tool-path overrides); first-class **native Rust fallback** driven by `SchemaCatalog` so the app fully works without client tools (especially Windows). Optional Postgres-tool sidecar bundling in Phase 6 (PostgreSQL license OK; MySQL tools are GPL — never bundled). |
| Frontend | **Svelte 5 + TypeScript** (runes), Vite 6 | Lighter/faster than React; Node 18 compatible (Vite 6). |
| MVP scope | **Full transfer engine first** | Phases 1–3 deliver a fully useful CLI before any UI work; Phases 4–5 layer the desktop on the same core. |
| Database drivers | **sqlx 0.8** (runtime-tokio, rustls TLS) | Async, pure Rust, all three engines. |
| SSH tunnels | **russh** (pure Rust) | Identical behavior on all three OSes, no OpenSSH dependency. (Later in Phase 1.) |
| Config sharing | `connections.toml` in platform config dir | `~/.config/mydb` (Linux), `~/Library/Application Support/mydb` (macOS), `%APPDATA%\mydb` (Windows); `MYDB_CONFIG_DIR`/`MYDB_CONFIG` overrides for tests. CLI and desktop see the same profiles. |
| Secrets | OS keyring via `keyring` crate, ChaCha20-Poly1305 file fallback | Passwords are stripped from `connections.toml` on save. |
| Progress reporting | One `TransferEvent` type over `tokio::sync::broadcast` | CLI renders progress bars (indicatif); Tauri forwards the same events to the UI. |
| Desktop grid/editor | TanStack Virtual + CodeMirror 6 (Phase 4) | Virtualized data grid; SQL completion sourced from live `SchemaCatalog`. |

### Safety rails

- Destructive modes (`clean-recreate`, `truncate-reload`) against a
  connection tagged `--env prod` require explicit confirmation
  (`-y` / typed phrase in the UI).
- `--dry-run` everywhere; the desktop wizard shows a full plan-review
  screen before execution.
- Post-transfer verification is on by default (`--skip-verify` to disable).

### Transfer modes

| Mode | Behavior |
|---|---|
| `clean-recreate` (default) | Drop and rebuild everything on the target |
| `truncate-reload` | Keep target schema, truncate tables, reload data |
| `diff-apply` | Apply only schema deltas (+ data for new tables) |

### CLI surface

```
mydb conn add|list|test|rm
mydb ls <conn> [--schema x]
mydb query <conn> "SELECT ..." [--format table|csv|json]
mydb dump <conn> [-o file] [--format custom|sql|directory] [--exclude-table ...]
mydb restore <file> --into <conn> [--clean] [--dry-run]
mydb transfer <src> <dst> [--mode ...] [--exclude-table ...] [--schema-only] [--dry-run] [-y]
mydb diff <a> <b> [--sql]
mydb verify <a> <b>
```

## Cross-platform notes

- **Tool detection**: dump engine detects `pg_dump`/`mysqldump` on PATH or
  per-profile override; UI shows detection status ("pg_dump 16.2 found ✓" /
  "not found — using built-in engine").
- **Paths**: `std::path` everywhere; temp files in platform temp dir.
- **Windows**: no `SIGTERM` — cancel kills child processes directly;
  `.exe` resolution for external tools; unsigned builds show SmartScreen
  warnings (signing optional).
- **Line endings**: generated SQL uses LF; `--crlf` opt-in later if needed.
- **WebView**: WKWebView (macOS), WebKitGTK (Linux, deps in .deb/.rpm),
  WebView2 (Windows, auto-bootstrapped by installer).
- **Release artifacts (Phase 6)**: macOS `.dmg` (aarch64 + x86_64),
  Windows `.msi` + NSIS `.exe`, Linux `.deb`/`.rpm`/`.AppImage`, via
  `tauri-action`.

## Non-goals (for now)

- NoSQL engines, SQL Server/Oracle (trait is open for later).
- Real-time collaboration / multi-user editing.
- Mobile apps (Tauri mobile scaffolding exists but is not a target).
- Bundling MySQL tools (GPL).
