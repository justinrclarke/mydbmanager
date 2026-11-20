export type EnvKind = "prod" | "staging" | "dev" | "test" | "other";

export function envKind(env: string): EnvKind {
  const s = env.toLowerCase();
  if (s.includes("prod")) return "prod";
  if (s.includes("stag")) return "staging";
  if (s.includes("dev")) return "dev";
  if (s.includes("test")) return "test";
  return "other";
}

export function envLabel(env: string): string {
  return env.replace(/^[^\w]+/, "").trim() || env;
}

export function kindShort(kind: string): string {
  switch (kind) {
    case "postgres":
      return "PG";
    case "mysql":
      return "MY";
    case "sqlite":
      return "QL";
    default:
      return kind.slice(0, 2).toUpperCase();
  }
}

export const NAV: { id: "browse" | "query" | "transfer" | "diff" | "verify"; label: string }[] = [
  { id: "browse", label: "Browse" },
  { id: "query", label: "SQL" },
  { id: "transfer", label: "Transfer" },
  { id: "diff", label: "Diff" },
  { id: "verify", label: "Verify" },
];
