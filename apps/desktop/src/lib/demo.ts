import type {
  AppInfo,
  ConnectionSummary,
  QueryResult,
  SchemaCatalog,
  SchemaDiff,
  Table,
  ToolStatus,
  TransferPlan,
  TransferPreset,
  VerificationReport,
} from "./types";

export function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

const users: Table = {
  name: "users",
  comment: null,
  columns: [
    { name: "id", ordinal: 1, data_type: "bigint", nullable: false, default: null, is_identity: true, identity_generation: "ALWAYS", comment: null },
    { name: "email", ordinal: 2, data_type: "text", nullable: false, default: null, is_identity: false, identity_generation: null, comment: null },
    { name: "created_at", ordinal: 3, data_type: "timestamptz", nullable: false, default: "now()", is_identity: false, identity_generation: null, comment: null },
  ],
  primary_key: { name: "users_pkey", columns: ["id"] },
  foreign_keys: [],
  checks: [],
  indexes: [{ name: "users_email_key", columns: ["email"], unique: true, partial: null }],
  triggers: [],
  row_estimate: 12840,
};

export const demoInfo: AppInfo = {
  appVersion: "0.1.0",
  coreVersion: "0.1.0",
  configPath: "/Users/you/.config/mydb/connections.toml",
};

export const demoConnections: ConnectionSummary[] = [
  { name: "prod-pg", env: "production", kind: "postgres", target: "db.prod.internal/app", ssh: "bastion" },
  { name: "staging-pg", env: "staging", kind: "postgres", target: "db.staging.internal/app", ssh: null },
  { name: "local-mysql", env: "dev", kind: "mysql", target: "127.0.0.1:3306/app", ssh: null },
  { name: "dev-sqlite", env: "dev", kind: "sqlite", target: "~/app.db", ssh: null },
];

export const demoCatalog: SchemaCatalog = {
  database: "app",
  provider: "postgres",
  captured_at: null,
  schemas: [
    {
      name: "public",
      tables: [
        users,
        {
          ...users,
          name: "orders",
          columns: [
            { name: "id", ordinal: 1, data_type: "bigint", nullable: false, default: null, is_identity: true, identity_generation: "ALWAYS", comment: null },
            { name: "user_id", ordinal: 2, data_type: "bigint", nullable: false, default: null, is_identity: false, identity_generation: null, comment: null },
            { name: "total", ordinal: 3, data_type: "numeric", nullable: false, default: null, is_identity: false, identity_generation: null, comment: null },
          ],
          primary_key: { name: "orders_pkey", columns: ["id"] },
          row_estimate: 90412,
        },
      ],
      views: [{ name: "active_users", definition: "", materialized: false }],
      materialized_views: [],
      sequences: [{ name: "users_id_seq", last_value: 12840, owned_by: ["public", "users"] }],
      functions: [{ name: "touch_updated_at", language: "plpgsql", definition: "", args: [] }],
      enums: [],
      extensions: [],
    },
  ],
};

export const demoRows: QueryResult = {
  columns: [
    { name: "id", type_name: "int8" },
    { name: "email", type_name: "text" },
    { name: "created_at", type_name: "timestamptz" },
  ],
  rows: [
    [{ int: 1 }, { text: "ada@example.com" }, { datetime: "2026-01-12T09:04:00Z" }],
    [{ int: 2 }, { text: "linus@example.com" }, { datetime: "2026-03-02T14:22:00Z" }],
    [{ int: 3 }, { null: null }, { datetime: "2026-06-18T08:11:00Z" }],
  ],
  rows_affected: 3,
  duration: 12,
};

export const demoTools: ToolStatus[] = [
  { backend: "pg_dump", path: "/opt/homebrew/bin/pg_dump", version: "16.4", statusLine: "pg_dump 16.4" },
];

export const demoPresets: TransferPreset[] = [
  { name: "prod-to-staging", source: "prod-pg", target: "staging-pg", mode: "clean-recreate", exclude_tables: [], schema_only: false },
];

export const demoPlan: TransferPlan = {
  mode: "clean-recreate",
  to_drop: [{ kind: "table", schema: "public", name: "scratch" }],
  to_create: [{ kind: "table", schema: "public", name: "orders" }],
  to_copy: [
    { schema: "public", table: "users", est_rows: 12840 },
    { schema: "public", table: "orders", est_rows: 90412 },
  ],
  sequences_to_set: [{ kind: "sequence", schema: "public", name: "users_id_seq" }],
  warnings: ["Target env is staging — existing data will be replaced."],
};

export const demoDiff: SchemaDiff = {
  items: [
    {
      kind: "table",
      action: "create",
      schema: "public",
      name: "orders",
      detail: "missing on target",
      sql: "CREATE TABLE public.orders (\n  id bigint PRIMARY KEY,\n  user_id bigint NOT NULL\n);",
    },
  ],
};

export const demoReport: VerificationReport = {
  source_counts: { tables: 2, sequences: 1 },
  target_counts: { tables: 2, sequences: 1 },
  row_mismatches: [],
  sequence_mismatches: [],
  passed: true,
};
