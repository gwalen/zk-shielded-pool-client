#!/usr/bin/env bash
# Copy the program IDL into idls/ (the declare_program! input).
# Run `anchor build` in the program repo first (or `just program-build`).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
program_repo="${ZK_POOL_PROGRAM_REPO:-$root/../pool-anchor-v2-rewrite}"
src="$program_repo/target/idl/zk_shielded_pool_solana.json"
dst="$root/idls/zk_shielded_pool_solana.json"

if [[ ! -f "$src" ]]; then
    echo "error: $src not found. Run \`anchor build\` in $program_repo first." >&2
    exit 1
fi

mkdir -p "$(dirname "$dst")"
if cmp -s "$src" "$dst"; then
    echo "IDL unchanged: $dst"
else
    cp "$src" "$dst"
    echo "IDL updated: $dst (rebuild and run \`cargo test\`; the discriminator tests catch a stale copy)"
fi
