import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppInfo,
  ConnectionSummary,
  QueryResult,
  SchemaCatalog,
  SchemaDiff,
  ToolStatus,
  TransferPlan,
  TransferPreset,
  VerificationReport,
} from "./types";

export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),
  listConnections: () => invoke<ConnectionSummary[]>("list_connections"),
  addConnection: (conn: Record<string, unknown>) => invoke<void>("add_connection", { conn }),
  removeConnection: (name: string) => invoke<void>("remove_connection", { name }),
  testConnection: (name: string) => invoke<string>("test_connection", { name }),
  introspect: (name: string) => invoke<SchemaCatalog>("introspect", { name }),
  executeSql: (name: string, sql: string) => invoke<QueryResult>("execute_sql", { name, sql }),
  fetchRows: (name: string, schema: string, table: string, limit: number, offset: number) =>
    invoke<QueryResult>("fetch_rows", { name, schema, table, limit, offset }),
  detectTools: (name: string) => invoke<ToolStatus[]>("detect_profile_tools", { name }),
  previewPlan: (source: string, target: string, options: Record<string, unknown>) =>
    invoke<TransferPlan>("preview_plan", { source, target, options }),
  runTransfer: (source: string, target: string, options: Record<string, unknown>) =>
    invoke<VerificationReport>("run_transfer", { source, target, options }),
  runDiff: (source: string, target: string) => invoke<SchemaDiff>("run_diff", { source, target }),
  runVerify: (source: string, target: string) =>
    invoke<VerificationReport>("run_verify", { source, target }),
  saveMigration: (source: string, target: string, dir: string) =>
    invoke<string>("save_migration", { source, target, dir }),
  listPresets: () => invoke<TransferPreset[]>("list_presets"),
  savePreset: (preset: TransferPreset) => invoke<void>("save_preset", { preset }),
};

export function onTransferEvent(
  handler: (event: { kind: string; message: string; table?: string; rows_done?: number; rows_total?: number }) => void,
): Promise<UnlistenFn> {
  return listen("transfer-event", (e) => handler(e.payload as never));
}

export function displayValue(v: unknown): string {
  if (v === null || v === undefined) return "NULL";
  if (typeof v !== "object") return String(v);
  const o = v as Record<string, unknown>;
  if ("null" in o) return "NULL";
  if ("bool" in o) return String(o.bool);
  if ("int" in o) return String(o.int);
  if ("uint" in o) return String(o.uint);
  if ("float" in o) return String(o.float);
  if ("numeric" in o) return String(o.numeric);
  if ("text" in o) return String(o.text);
  if ("json" in o) return String(o.json);
  if ("uuid" in o) return String(o.uuid);
  if ("datetime" in o) return String(o.datetime);
  if ("bytes" in o) return `<${(o.bytes as number[]).length} bytes>`;
  return JSON.stringify(v);
}
