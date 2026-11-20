# Milestones & Progress

> Check items off as they land. Keep the status table at top current —
> AI agents and contributors read this first.

## Status overview

| Phase | Deliverable | Status |
|---|---|---|
| 0 | Workspace scaffold, CI matrix (macOS/Windows/Linux) | ✅ Done |
| 1 | Connections, introspection (PG/MySQL/SQLite), `mydb conn test/ls/query` | ✅ Done |
| 2 | Hybrid dump/restore + native fallback + verification; `mydb dump/restore/verify` | ✅ Done |
| 3 | Transfer + diff engines; `mydb transfer/diff` with dry-run + progress events | ✅ Done |
| 4 | Desktop: sidebar, schema tree, data grid, SQL editor | ✅ Done |
| 5 | Desktop: transfer wizard, diff viewer, verification reports | ✅ Done |
| 6 | Migration export, presets, sidecar bundling, release packaging | ✅ Done |

Legend: ✅ done · 🔨 in progress · ⬜ not started

---

## Phase 0 — Scaffold ✅

- [x] Cargo workspace: `crates/core`, `crates/cli`, `apps/desktop/src-tauri`
- [x] `mydb-core`: error types, `DatabaseProvider` trait + `Capabilities`,
      `SchemaCatalog` object model, connection profile store (TOML),
      transfer/dump/diff/query type skeletons, `VerificationReport` model
- [x] CLI: `mydb conn add/list/rm` working (env badges, password masking,
      URL kind inference, SSH fields); other subcommands stubbed with
      phase markers
- [x] Desktop: Tauri v2 + Svelte 5 + TS + Vite 6 scaffold, app shell with
      connection sidebar, generated icons
- [x] CI: fmt / clippy `-D warnings` / test on ubuntu+macos+windows;
      frontend check + build
- [x] 16 unit tests green, clippy clean, frontend type-checks

## Phase 1 — Connections & Introspection ✅

- [x] Provider framework: `providers::connect()` factory, `build_url()`
      from profile parts (URL escaping, SQLite paths)
- [x] `server_version()` on the trait
- [x] **PostgresProvider**: full `information_schema`/`pg_catalog`
      introspection (tables, columns w/ identity+generated, PK/FK/CHECK,
      indexes incl. partial+method, triggers w/ tgtype decoding, sequences
      w/ `last_value`+ownership via `pg_depend`, views, matviews, functions
      w/ full `pg_get_functiondef`, enums, extensions) + typed value mapping
- [x] **MysqlProvider**: introspection via `information_schema` (tables,
      columns w/ auto_increment, PK/FK/CHECK, indexes, triggers, views,
      routines) + value mapping
- [x] **SqliteProvider**: `sqlite_master` + PRAGMAs (table_info,
      foreign_key_list, index_list/index_info), trigger SQL parsing,
      CHECK parsing, value mapping
- [x] Integration tests vs real servers (Docker: `postgres:16` on :55432,
      `mysql:8` on :33306; temp-file SQLite) with a rich fixture schema
      covering every object type
- [x] CLI `mydb conn test` — connect + print server version
- [x] CLI `mydb ls` — tree of schemas → object kinds → names (row
      estimates); `--schema` filter
- [x] CLI `mydb query` — table (comfy-table) / csv / json output
- [x] SSH tunneling via `russh` (local port-forward through bastion,
      wired into connect path; `SshConfig` already in profile store)
- [x] Secrets in OS keyring (`keyring` crate) with encrypted-file fallback;
      remove plaintext passwords from `connections.toml`
- [x] CI: Postgres + MySQL service containers, integration tests run on
      Linux job

## Phase 2 — Dump / Restore ✅

- [x] Tool detection: PATH scan + per-profile overrides; surfaced to CLI
      (`--backend`) and UI
- [x] Postgres via `pg_dump -Fc` / `pg_restore --clean --if-exists --no-owner`
- [x] MySQL via `mysqldump --single-transaction --routines --triggers
      --events --set-gtid-purged=OFF` / `mysql` restore
- [x] SQLite via native engine (VACUUM INTO / SQL replay)
- [x] Native Rust fallback engine driven by `SchemaCatalog`:
      dependency-ordered DDL (enums/types → sequences → tables →
      constraints → indexes → views → functions → triggers), batched
      INSERT, `setval()` after data load
- [x] Parity / round-trip tests: native dump → restore → verify on SQLite
- [x] `mydb dump` (custom/sql/directory formats, --schema-only, --clean,
      --exclude-table globs)
- [x] `mydb restore` (--clean, --dry-run plan preview)
- [x] Verification report after restore (object counts, row counts,
      sequence values)
- [x] Structured progress events for long dumps/restores

## Phase 3 — Transfer & Diff ✅

- [x] Transfer orchestrator: introspect → plan → apply → verify → report,
      honoring include/exclude globs, `schema_only`, batch size
- [x] `TransferPlan` builder per mode (clean-recreate / truncate-reload /
      diff-apply) with FK-safe copy order
- [x] Dry-run plan output (CLI text + serializable for UI)
- [x] Production guard: refuse destructive mode on `env=production`
      target without explicit confirmation
- [x] Progress streaming via `tokio::sync::broadcast<TransferEvent>`
- [x] Diff engine: catalog comparison → `DiffItem` list → migration SQL
      generation (CREATE/ALTER/DROP per object type)
- [x] `mydb transfer` with live progress
- [x] `mydb diff --sql` + save-as-migration
- [x] `mydb verify` standalone
- [x] End-to-end test: SQLite fixture → transfer → verify passes

## Phase 4 — Desktop: Browse & Query ✅

- [x] Tauri command layer over core (async, streaming events)
- [x] Connection sidebar: env badges, connect/disconnect, add/edit forms
- [x] Schema tree: schemas → tables/views/sequences/functions/triggers/
      enums/extensions
- [x] Object detail views: columns, indexes, constraints, DDL, sequence
      current value, function definitions
- [x] Data grid: TanStack Virtual, paging, NULL handling
- [x] SQL editor: CodeMirror 6, multi-tab-ready result grid

## Phase 5 — Desktop: Transfer & Diff UI ✅

- [x] Transfer wizard: source → target → mode/options → **plan review**
      → live progress → verification report
- [x] Diff viewer: side-by-side change list + generated SQL
- [x] Verification report screen
- [x] Tool-detection status panel (pg_dump found/version)

## Phase 6 — Polish & Release ✅

- [x] Migration file export (timestamped, diff-derived)
- [x] Saved transfer presets (one-click "prod → dev")
- [x] Postgres tool sidecar bundling script (`scripts/bundle-pg-tools.sh`)
- [x] Release CI via `tauri-action`: `.dmg` (aarch64+x86_64),
      `.msi`/NSIS, `.deb`/`.rpm`/`.AppImage`
- [x] Smoke tests: CLI `--help`, desktop `dist/index.html` check
      (`tauri-driver` notes in the smoke script)
- [x] App signing groundwork (macOS hardened runtime / identity slot,
      Windows timestamp URL)
