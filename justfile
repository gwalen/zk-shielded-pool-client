program_id := "FCrymxYUTEnXDJXdDn2E71KPxB9sBjXZCh1ezBwmUhvp"
rpc_url := "http://127.0.0.1:8899"

# Defaults for `just it`: offline, no mainnet fork, with the SIMD-0529 sol_big_mod_exp syscall
# (feature expH2pp…) that the withdraw verifier needs. Requires Surfpool >= 1.7.0 (Agave 4.3).
# The feature is not active on mainnet/devnet yet, so this is NOT a mainnet check.
# Override from the shell, e.g. `SURFPOOL_OFFLINE= SURFPOOL_FEATURES= just it` for a plain mainnet fork.
export SURFPOOL_OFFLINE := env("SURFPOOL_OFFLINE", "1")
export SURFPOOL_FEATURES := env("SURFPOOL_FEATURES", "expH2ppKPW2ANEdEmAjfhSEcnBQJfmoX4FjuNpe9ttg")

# Build the program in the sibling repo, then copy its IDL into idls/.
program-build:
    cd ../pool-anchor-v2-rewrite && NO_DNA=1 anchor build
    scripts/sync-idl.sh

# Start a surfnet (mainnet fork) in this terminal for manual runs with `cargo run`.
# Pair with `just it-surfpool` to run the integration tests against it.
surfpool:
    surfpool start --no-tui

# Integration tests in one command: start Surfpool, wait for the deployment runbook, run the
# Rust integration tests, always stop Surfpool.
#   SURFPOOL_OFFLINE=1   no mainnet fork (fast, no network, same mainnet feature set)
#   SURFPOOL_FEATURES=a,b extra --feature flags (NOT a mainnet check)
# Both default to the values at the top of this file.
it:
    #!/usr/bin/env bash
    set -euo pipefail
    # surfpool inherits this shell's stdout/stderr, so its logs would interleave with the
    # test output. Send them to a file instead and only surface it on failure.
    log=target/surfpool.log
    mkdir -p "$(dirname "$log")"
    : > "$log"
    args=(--yes --no-tui --no-studio)
    if [[ "${SURFPOOL_OFFLINE:-}" == "1" ]]; then
        args+=(--offline)
        echo "surfpool: offline (no mainnet fork)"
    else
        echo "surfpool: forking mainnet"
    fi
    if [[ -n "${SURFPOOL_FEATURES:-}" ]]; then
        IFS=',' read -ra features <<< "${SURFPOOL_FEATURES}"
        for feature in "${features[@]}"; do args+=(--feature "$feature"); done
        echo "WARNING: extra features enabled (${SURFPOOL_FEATURES}): this run is NOT a mainnet check" >&2
    fi
    NO_DNA=1 surfpool start "${args[@]}" >"$log" 2>&1 &
    surfpool_pid=$!
    trap 'kill ${surfpool_pid} 2>/dev/null || true' EXIT
    echo "surfpool logs -> $log"
    until curl -s {{rpc_url}} >/dev/null; do
        if ! kill -0 ${surfpool_pid} 2>/dev/null; then
            echo "error: surfpool exited early, last lines of $log:" >&2
            tail -n 40 "$log" >&2
            exit 1
        fi
        sleep 0.2
    done
    # The RPC answers before the deployment runbook finishes (longer with a mainnet fork).
    just wait-for-deployment
    just _it-tests

# Integration tests against a surfnet already running (`just surfpool` in another terminal).
it-surfpool:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! curl -s {{rpc_url}} >/dev/null; then
        echo "error: surfpool is not reachable at {{rpc_url}}" >&2
        echo "start it in another terminal with: just surfpool" >&2
        exit 1
    fi
    just wait-for-deployment
    just _it-tests

# Two commands: Cargo orders test binaries by name, and the smoke test must see the fresh
# surfnet first. Deposit/withdraw share one tree, so they run on one thread.
[private]
_it-tests:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo test --test it_smoke -- --ignored
    tests=()
    for t in it_deposit it_withdraw; do
        [[ -f "tests/integration_tests/$t.rs" ]] && tests+=(--test "$t")
    done
    if (( ${#tests[@]} )); then
        cargo test "${tests[@]}" -- --ignored --test-threads=1
    fi

# Block until the deployment runbook made the program executable on the surfnet (max 60 s).
wait-for-deployment:
    #!/usr/bin/env bash
    set -euo pipefail
    for _ in $(seq 1 120); do
        if curl -s {{rpc_url}} -X POST -H 'Content-Type: application/json' \
            -d '{"jsonrpc":"2.0","id":1,"method":"getAccountInfo","params":["{{program_id}}",{"encoding":"base64"}]}' \
            | grep -q '"executable":true'; then
            exit 0
        fi
        sleep 0.5
    done
    echo "error: {{program_id}} was not deployed on the surfnet after 60s (see target/surfpool.log)" >&2
    exit 1
