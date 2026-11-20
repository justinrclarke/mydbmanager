#!/usr/bin/env node
// Lightweight smoke check used by CI in place of a full tauri-driver run.
// Full WebDriver coverage: `npm run tauri driver` + a WebDriver client per OS.
import { readFileSync, existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const index = join(root, "dist", "index.html");
if (!existsSync(index)) {
  console.error("smoke: dist/index.html missing — run npm run build first");
  process.exit(1);
}
const html = readFileSync(index, "utf8");
if (!html.includes("MyDB Manager")) {
  console.error("smoke: built index.html does not contain app title");
  process.exit(1);
}
console.log("smoke: desktop build artifact looks healthy");
