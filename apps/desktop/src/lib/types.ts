export interface AppInfo {
  appVersion: string;
  coreVersion: string;
  configPath: string;
}

export interface ConnectionSummary {
  name: string;
  env: string;
  kind: string;
  target: string;
  ssh: string | null;
}

export interface Column {
  name: string;
  ordinal: number;
  data_type: string;
  nullable: boolean;
  default: string | null;
  is_identity: boolean;
  identity_generation: string | null;
  comment: string | null;
}

export interface Table {
  name: string;
  comment: string | null;
  columns: Column[];
  primary_key: { name: string | null; columns: string[] } | null;
  foreign_keys: {
    name: string;
    columns: string[];
    ref_schema: string;
    ref_table: string;
    ref_columns: string[];
    on_delete: string | null;
  }[];
  checks: { name: string; expression: string }[];
  indexes: { name: string; columns: string[]; unique: boolean; partial: string | null }[];
  triggers: { name: string; timing: string; events: string[]; definition: string }[];
  row_estimate: number | null;
}

export interface Schema {
  name: string;
  tables: Table[];
  views: { name: string; definition: string; materialized: boolean }[];
  materialized_views: { name: string; definition: string; materialized: boolean }[];
  sequences: {
    name: string;
    last_value: number | null;
    owned_by: [string, string] | null;
  }[];
  functions: { name: string; language: string; definition: string; args: string[] }[];
  enums: { name: string; labels: string[] }[];
  extensions: { name: string; version: string | null }[];
}

export interface SchemaCatalog {
  database: string;
  provider: string;
  captured_at: string | null;
  schemas: Schema[];
}

export type SqlValue =
  | { null: null }
  | { bool: boolean }
  | { int: number }
  | { uint: number }
  | { float: number }
  | { numeric: string }
  | { text: string }
  | { bytes: number[] }
  | { json: string }
  | { uuid: string }
  | { datetime: string };

export interface QueryResult {
  columns: { name: string; type_name: string }[];
  rows: SqlValue[][];
  rows_affected: number;
  duration: number;
}

export interface TransferPlan {
  mode: string;
  to_drop: { kind: string; schema: string; name: string }[];
  to_create: { kind: string; schema: string; name: string }[];
  to_copy: { schema: string; table: string; est_rows: number }[];
  sequences_to_set: { kind: string; schema: string; name: string }[];
  warnings: string[];
}

export interface VerificationReport {
  source_counts: Record<string, number>;
  target_counts: Record<string, number>;
  row_mismatches: [string, number, number][];
  sequence_mismatches: [string, number | null, number | null][];
  passed: boolean;
}

export interface DiffItem {
  kind: string;
  action: string;
  schema: string;
  name: string;
  detail: string;
  sql?: string | null;
}

export interface SchemaDiff {
  items: DiffItem[];
}

export interface ToolStatus {
  backend: string;
  path: string;
  version: string;
  statusLine: string;
}

export interface TransferPreset {
  name: string;
  source: string;
  target: string;
  mode: string;
  exclude_tables: string[];
  schema_only: boolean;
}

export type ViewId = "browse" | "query" | "transfer" | "diff" | "verify";
