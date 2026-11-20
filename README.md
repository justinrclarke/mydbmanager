# MyDB Manager

Move databases between environments — production → staging → dev — **without
losing anything**. Tables, sequences (including their current values), views,
functions, triggers, indexes, enums and constraints: MyDB captures the full
object graph, replays it in dependency order, and verifies the result.

Two frontends share one Rust engine:

- **`mydb`** — a terminal CLI
- **MyDB Manager** — a cross-platform desktop app (Tauri v2 + Svelte 5) for
  macOS, Linux and Windows

Supports **PostgreSQL**, **MySQL/MariaDB** and **SQLite**.

📖 **[Project plan](docs/PLAN.md)** · **[Milestones & progress](docs/MILESTONES.md)** · **[Contributor/AI-agent guide](AGENTS.md)**

## Architecture

```
mydbmanager/
├── crates/
│   ├── core/        # mydb-core: all database logic, zero UI
│   │   ├── conn/        # shared connection profiles (TOML, platform config dir)
│   │   ├── introspect/  # SchemaCatalog: full typed object graph
│   │   ├── provider.rs  # DatabaseProvider trait + capabilities per engine
│   │   ├── query/       # ad-hoc query results
│   │   ├── dump/        # hybrid dump/restore engine
│   │   ├── transfer/    # env → env transfer + verification reports
│   │   └── diff/        # schema diff + migration SQL
│   └── cli/         # `mydb` binary (clap)
└── apps/
    └── desktop/     # Tauri v2 app (Svelte 5 + TypeScript frontend)
        └── src-tauri/
```

The CLI and desktop app share the same connection profiles
(`~/.config/mydb/connections.toml`, `~/Library/Application Support/mydb`,
or `%APPDATA%\mydb`).

### Why dumps lose things (and how this doesn't)

Naïve dump/import flows miss sequences, leave `last_value` at 1 (so fresh
inserts collide with restored rows), or create objects in the wrong order.
MyDB's engine:

1. Introspects the **complete** catalog from the source (including sequence
   `last_value`s and ownership).
2. Emits DDL in dependency order: types/enums → sequences → tables →
   constraints → indexes → views → functions → triggers.
3. Copies data in FK-safe order, then runs `setval()` for every sequence.
4. **Verifies**: re-introspects the target and compares object counts,
   per-table row counts and sequence values, producing a report.

Dumps use `pg_dump`/`mysqldump` when detected (with per-connection tool path
overrides), and a native Rust engine otherwise — so the app is fully
functional even on machines without database client tools installed.

## CLI

```bash
mydb conn add prod --url postgres://user:pass@host:5432/app --env prod
mydb conn add dev --url postgres://user:pass@localhost:5432/app --env dev
mydb conn list

mydb ls prod --schema public
mydb query dev "SELECT count(*) FROM users"

mydb dump prod -o prod.dump --format custom
mydb restore prod.dump --into dev --clean

mydb transfer prod dev --mode clean-recreate --dry-run
mydb transfer prod dev --mode clean-recreate -y
mydb diff prod staging --sql
mydb verify prod dev
```

Transfer modes:

| Mode | Behavior |
|---|---|
| `clean-recreate` (default) | Drop and rebuild everything on the target |
| `truncate-reload` | Keep target schema, truncate tables, reload data |
| `diff-apply` | Apply only schema deltas (+ data for new tables) |

Destructive modes against a connection tagged `--env prod` require explicit
confirmation (`-y` / typed phrase in the UI).

## Development

Prerequisites:

- Rust ≥ 1.85 (`rustup`)
- Node ≥ 18 (Vite 6) — Node 20+ recommended
- Linux only: `libwebkit2gtk-4.1-dev build-essential libssl-dev
  libayatana-appindicator3-dev librsvg2-dev` (see `.github/workflows/ci.yml`)

```bash
# engine + CLI
cargo test --workspace
cargo run -p mydb -- conn list

# desktop app (dev, with hot reload)
cd apps/desktop
npm install
npm run tauri dev

# regenerate app icons
npm run icon:generate
npm run tauri icon app-icon.png
```

## Roadmap

| Phase | Deliverable | Status |
|---|---|---|
| 0 | Workspace scaffold, CI matrix (macOS/Windows/Linux) | ✅ |
| 1 | Connections (profiles, TLS, SSH tunnels), introspection (PG/MySQL/SQLite), `mydb conn/ls/query` | ✅ |
| 2 | Hybrid dump/restore + native fallback + verification; `mydb dump/restore/verify` | ✅ |
| 3 | Transfer + diff engines; `mydb transfer/diff` with dry-run + progress events | ✅ |
| 4 | Desktop: sidebar, schema tree, data grid, SQL editor | ✅ |
| 5 | Desktop: transfer wizard, diff viewer, verification reports | ✅ |
| 6 | Migration export, transfer presets, tool sidecar bundling, release packaging (.dmg/.msi/.deb/.rpm/.AppImage) | ✅ |

## License

MIT OR Apache-2.0
