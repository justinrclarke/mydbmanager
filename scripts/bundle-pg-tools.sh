#!/usr/bin/env bash
# Optional Postgres tool sidecar for desktop bundles.
# PostgreSQL client tools are under a permissive license and MAY be bundled.
# MySQL tools are GPL — never copy mysqldump/mysql into this tree.
set -euo pipefail
dest="${1:-apps/desktop/src-tauri/sidecars}"
mkdir -p "$dest"
for tool in pg_dump pg_restore; do
  path="$(command -v "$tool" || true)"
  if [[ -n "$path" ]]; then
    cp "$path" "$dest/"
    echo "copied $path -> $dest/"
  else
    echo "skip: $tool not on PATH"
  fi
done
echo "Add the copied binaries to tauri.conf.json bundle.externalBin when shipping a sidecar build."
