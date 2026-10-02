#!/usr/bin/env bash
# Enforces the consensus-core layering rules from docs/adr/0001-foundational-choices.md.
set -euo pipefail

cd "$(dirname "$0")/.."

CORE_CRATES=(yuanlai-codec yuanlai-crypto yuanlai-types yuanlai-zk yuanlai-consensus)
FORBIDDEN_DEPS=(tokio mio async-std smol hyper axum reqwest redb rocksdb yuanlai-storage yuanlai-mempool yuanlai-p2p yuanlai-miner yuanlai-rpc)

fail=0

for crate in "${CORE_CRATES[@]}"; do
    if ! cmp -s tools/core-clippy.toml "crates/$crate/clippy.toml"; then
        echo "error: crates/$crate/clippy.toml differs from tools/core-clippy.toml"
        fail=1
    fi

    deps="$(cargo tree --locked -p "$crate" -e normal --prefix none --format '{p}' | awk '{print $1}' | sort -u)"
    for forbidden in "${FORBIDDEN_DEPS[@]}"; do
        if grep -qx "$forbidden" <<<"$deps"; then
            echo "error: core crate $crate depends on forbidden crate $forbidden"
            fail=1
        fi
    done
done

if [[ $fail -ne 0 ]]; then
    exit 1
fi
echo "core layering check passed"
