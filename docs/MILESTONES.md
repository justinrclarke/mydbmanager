# Milestones & Progress

> Check items off as they land. Keep the status table at top current —
> AI agents and contributors read this first.

## Status overview

| Phase | Deliverable | Status |
|---|---|---|
| 0 | Workspace scaffold, CI matrix (macOS/Windows/Linux) | ✅ Done (commit `1d33678`) |
| 1 | Connections, introspection (PG/MySQL/SQLite), `mydb conn test/ls/query` | 🔨 In progress |
| 2 | Hybrid dump/restore + native fallback + verification; `mydb dump/restore/verify` | ⬜ |
| 3 | Transfer + diff engines; `mydb transfer/diff` with dry-run + progress events | ⬜ |
| 4 | Desktop: sidebar, schema tree, data grid, SQL editor | ⬜ |
| 5 | Desktop: transfer wizard, diff viewer, verification reports | ⬜ |
| 6 | Migration export, presets, sidecar bundling, release packaging | ⬜ |

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

## Phase 1 — Connections & Introspection 🔨

- [x] Provider framework: `providers::connect()` factory, `build_url()`
      from profile parts (URL escaping, SQLite paths)
- [x] `server_version()` on the trait
- [ ] **PostgresProvider**: full `information_schema`/`pg_catalog`
      introspection (tables, columns w/ identity+generated, PK/FK/CHECK,
      indexes incl. partial+method, triggers w/ tgtype decoding, sequences
      w/ `last_value`+ownership via `pg_depend`, views, matviews, functions
      w/ full `pg_get_functiondef`, enums, extensions) + typed value mapping
      — *written, needs compile fix + live test*
- [ ] **MysqlProvider**: introspection via `information_schema` (tables,
      columns w/ auto_increment, PK/FK/CHECK, indexes, triggers, views,
      routines) + value mapping — *written, needs live test*
- [ ] **SqliteProvider**: `sqlite_master` + PRAGMAs (table_info,
      foreign_key_list, index_list/index_info), trigger SQL parsing,
      value mapping — *written, needs tests*
- [ ] Integration tests vs real servers (Docker: `postgres:16` on :55432,
      `mysql:8` on :33306; temp-file SQLite) with a rich fixture schema
      covering every object type
- [ ] CLI `mydb conn test` — connect + print server version
- [ ] CLI `mydb ls` — tree of schemas → object kinds → names (row
      estimates); `--schema` filter
- [ ] CLI `mydb query` — table (comfy-table) / csv / json output
- [ ] SSH tunneling via `russh` (local port-forward through bastion,
      wired into connect path; `SshConfig` already in profile store)
- [ ] Secrets in OS keyring (`keyring` crate) with encrypted-file fallback;
      remove plaintext passwords from `connections.toml`
- [ ] CI: Postgres + MySQL service containers, integration tests run on
      Linux job

## Phase 2 — Dump / Restore

- [ ] Tool detection: PATH scan + per-profile overrides + server/tool
      version compatibility check; surfaced to CLI (`--backend`) and UI
- [ ] Postgres via `pg_dump -Fc` / `pg_restore --clean --if-exists --no-owner`
- [ ] MySQL via `mysqldump --single-transaction --routines --triggers
      --events --set-gtid-purged=OFF` / `mysql` restore
- [ ] SQLite via native engine (VACUUM INTO / SQL replay)
- [ ] Native Rust fallback engine driven by `SchemaCatalog`:
      dependency-ordered DDL (enums/types → sequences → tables →
      constraints → indexes → views → functions → triggers), batched
      COPY/multi-row inserts, `setval()` after data load
- [ ] Parity checklist + tests: native output restores identically to
      tool output on the fixture schemas
- [ ] `mydb dump` (custom/sql/directory formats, --schema-only, --clean,
      --exclude-table globs)
- [ ] `mydb restore` (--clean, --dry-run plan preview)
- [ ] Verification report after restore (object counts, row counts,
      sequence values)
- [ ] Structured progress events for long dumps/restores

## Phase 3 — Transfer & Diff

- [ ] Transfer orchestrator: introspect → plan → apply → verify → report,
      honoring include/exclude globs, `schema_only`, batch size
- [ ] `TransferPlan` builder per mode (clean-recreate / truncate-reload /
      diff-apply) with FK-safe copy order
- [ ] Dry-run plan output (CLI text + serializable for UI)
- [ ] Production guard: refuse destructive mode on `env=production`
      target without explicit confirmation
- [ ] Progress streaming via `tokio::sync::broadcast<TransferEvent>`
- [ ] Diff engine: catalog comparison → `DiffItem` list → migration SQL
      generation (CREATE/ALTER/DROP per object type)
- [ ] `mydb transfer` with live progress bars (indicatif)
- [ ] `mydb diff --sql` + save-as-migration
- [ ] `mydb verify` standalone
- [ ] End-to-end test: prod-like fixture → transfer → verify passes;
      deliberate sequence drift detected

## Phase 4 — Desktop: Browse & Query

- [ ] Tauri command layer over core (async, streaming events)
- [ ] Connection sidebar: env badges, connect/disconnect, add/edit forms
- [ ] Schema tree: schemas → tables/views/sequences/functions/triggers/
      enums/extensions, lazy-loaded
- [ ] Object detail views: columns, indexes, constraints, DDL, sequence
      current value, function definitions
- [ ] Data grid: TanStack Virtual, server-side paging/sorting/filtering,
      inline edit w/ per-row save, NULL handling, FK jump-to-row
- [ ] SQL editor: CodeMirror 6, completion from live catalog, multi-tab,
      result grid + CSV/JSON export

## Phase 5 — Desktop: Transfer & Diff UI

- [ ] Transfer wizard: source → target → mode/options → **plan review**
      → live progress (per-table row counts) → verification report
- [ ] Diff viewer: side-by-side change list + generated SQL
- [ ] Verification report screen
- [ ] Tool-detection status panel (pg_dump found/version)

## Phase 6 — Polish & Release

- [ ] Migration file export (timestamped, diff-derived)
- [ ] Saved transfer presets (one-click "prod → dev")
- [ ] Postgres tool sidecar bundling (optional, per-platform)
- [ ] Release CI via `tauri-action`: `.dmg` (aarch64+x86_64),
      `.msi`/NSIS, `.deb`/`.rpm`/`.AppImage`
- [ ] `tauri-driver` smoke tests per OS
- [ ] App signing groundwork (macOS notarization, Windows optional)
