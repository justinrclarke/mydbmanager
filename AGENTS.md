# AGENTS.md — Guide for AI Agents & Contributors

Welcome to **MyDB Manager**. Read this first, then
[docs/PLAN.md](docs/PLAN.md) (architecture & decisions) and
[docs/MILESTONES.md](docs/MILESTONES.md) (what's done / what's next).

## What this is

A database manager for **PostgreSQL, MySQL/MariaDB and SQLite** with two
frontends over one Rust engine:

- **`mydb`** — terminal CLI (`crates/cli`)
- **MyDB Manager** — Tauri v2 desktop app (`apps/desktop`), Svelte 5 + TS

The killer feature: moving whole databases between environments
(prod → staging → dev) with **zero object loss** — including sequences and
their current values — plus post-transfer verification. See PLAN.md for why
naive dump/import fails at this.

## Repository layout

```
crates/core/            mydb-core — ALL database logic lives here. Zero UI deps.
  src/conn/             connection profiles (shared TOML store) + SSH config
  src/introspect/       SchemaCatalog: the typed object graph (central model!)
  src/provider.rs       DatabaseProvider trait + Capabilities per engine
  src/providers/        postgres.rs / mysql.rs / sqlite.rs implementations
  src/query/            driver-agnostic Value + QueryResult
  src/dump/             dump/restore options & backend types (Phase 2)
  src/transfer/         transfer modes, plan, events, VerificationReport (Phase 3)
  src/diff/             schema diff types (Phase 3)
crates/cli/             mydb binary (clap). Thin layer over core.
apps/desktop/src-tauri/ Tauri shell. Thin layer over core.
apps/desktop/src/       Svelte 5 frontend (runes, TS strict-ish)
docs/                   PLAN.md, MILESTONES.md
```

**Golden rule:** features are implemented in `mydb-core` first, then exposed
by BOTH the CLI and Tauri commands. Never put logic in the frontends.

## Commands

```bash
# Rust (from repo root)
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings   # must pass; CI enforces
cargo test --workspace
cargo run -p mydb -- conn list

# Frontend (from apps/desktop)
npm install
npm run check          # svelte-check
npm run build
npm run tauri dev      # run the desktop app (dev, hot reload)

# Test database servers (Docker)
docker run -d --name mydb-pg    -e POSTGRES_PASSWORD=test -e POSTGRES_DB=appdb -p 55432:5432 postgres:16
docker run -d --name mydb-mysql -e MYSQL_ROOT_PASSWORD=test -e MYSQL_DATABASE=appdb -p 33306:3306 mysql:8
```

Integration tests read `MYDB_TEST_PG_URL` / `MYDB_TEST_MYSQL_URL` and skip
when unset. Unit tests must not require network/servers.

## Conventions

- **Rust**: edition 2021, `#![forbid(unsafe_code)]` in core. All public
  items documented (`cargo doc` style `///`). Errors: `MyDbError`
  (thiserror) in core; `anyhow` only in the CLI binary. Async: tokio.
  DB access: sqlx 0.8 (rustls; no native TLS).
- **Provider code**: implement the `DatabaseProvider` trait; consult
  `Capabilities` instead of branching on `ProviderKind` in shared code.
  Map driver values to `query::Value` — never leak sqlx types past the
  provider boundary.
- **The SchemaCatalog is the source of truth.** Any new object type
  (e.g. Postgres domains, policies) gets: a struct in `introspect/`,
  an `ObjectType` variant, an entry in `ObjectCounts` (+ comparison in
  verification), introspection per provider, and DDL emission in the
  native dump engine. Missing one of these is how tools lose objects.
- **Progress/events**: long operations emit `transfer::TransferEvent` over
  a broadcast channel. CLI renders with indicatif; Tauri forwards via
  `app.emit`. Same event type, two renderers.
- **Svelte**: Svelte 5 runes (`$state`, `$derived`, `$effect`), no
  legacy stores for new code. TS in `.svelte` files (`lang="ts"`).
- **Commits**: imperative mood, prefix with the phase when applicable
  (e.g. `Phase 1: postgres introspection + ls/query wiring`). Update
  `docs/MILESTONES.md` checkboxes in the same commit that finishes work.

## Safety rules (non-negotiable)

1. Destructive operations (`clean-recreate`, `truncate-reload`, restore
   `--clean`) against a profile with `env = production` MUST require
   explicit confirmation (`-y` flag / typed phrase in UI).
2. Never log or print passwords — use `mask_url_password()` /
   `masked_target()`.
3. Tests must never target a real production-looking host; use the Docker
   containers, temp files, or `MYDB_TEST_*` env URLs.
4. Generated migration SQL is data: always LF line endings, never
   interpolated into shell commands (use `tokio::process` arg arrays).

## Current state (update as you work)

- **Phases 0–6 implemented**: core providers + SSH + keyring, native/hybrid
  dump-restore, transfer/diff/verify, CLI surface, desktop browse/query/
  transfer/diff UI, presets, migration export, CI DB services, release
  workflow. Remaining polish: host-key verification for SSH, MySQL
  `SHOW CREATE FUNCTION` argument parsing, live `tauri-driver` runs.
- Toolchain notes: Rust 1.95, Node 18 locally (Vite 6 constraint — do NOT
  bump to Vite 7 unless Node ≥ 20.19 is guaranteed), pg_dump 18 +
  mysqldump available on the maintainer's macOS machine.

## Known gaps / TODO markers

- SSH `check_server_key` currently accepts any host key (document for
  users; known_hosts store is the next hardening step).
- MySQL `Function.args` is empty — needs `SHOW CREATE FUNCTION` parsing.
- MySQL CHECK constraints: silently skipped on engines without
  `information_schema.check_constraints` (error 1109).
- SQLite `row_estimate` is None (no planner stats).

## For AI agents specifically

- Work in small, testable increments; run `cargo clippy --workspace
  --all-targets -- -D warnings` and `cargo test --workspace` before
  claiming done. CI enforces both on three OSes.
- When finishing a milestone item, tick its checkbox in
  `docs/MILESTONES.md` in the same commit.
- Prefer extending the provider trait + capabilities over
  engine-specific special cases in higher layers.
- The `SchemaCatalog` round-trip property is sacred: anything
  introspected must be restorable by the native engine (Phase 2 parity
  checklist).
