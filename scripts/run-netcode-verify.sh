#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
runtime_dir="${PLAYWRIGHT_RUNTIME_DIR:-${TMPDIR:-/tmp}/discordium-td-playwright-runtime}"

for command_name in node npm npx; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "missing required command: $command_name" >&2
    exit 1
  fi
done

mkdir -p "$runtime_dir"
if [[ ! -f "$runtime_dir/package.json" ]]; then
  (cd "$runtime_dir" && npm init -y >/dev/null 2>&1)
fi

if [[ ! -d "$runtime_dir/node_modules/playwright" ]]; then
  (cd "$runtime_dir" && npm install playwright >/dev/null)
fi

(cd "$runtime_dir" && npx playwright install chromium >/dev/null)

export PLAYWRIGHT_RUNTIME_DIR="$runtime_dir"
exec node "$script_dir/netcode-verify.mjs" "$@"
