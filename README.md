# Client for zk-shielded-pool contracts

## Setup

- Copy `.env.example` to `.env` and fill in the wallet keys.
  - `DEPOSIT_WALLET_PRIVATE_KEYS`: wallets that sign deposits and pay the funds and fees.
  - `WITHDRAW_WALLET_PRIVATE_KEYS`: fee payers and signers for `upload_proof`, `withdraw` and `close_proof`.
    The first one is the default.
  - A key may not be in both lists (startup fails). Logs show public keys only.
- Plain `cargo build` works from a clean checkout. `.cargo/config.toml` sets `RUSTC_BOOTSTRAP=1`, because the
  proof-generator dependency (vendored halo2-base, poseidon-primitives) uses nightly features. A Rust upgrade can
  break those without warning.
- The program interface is generated from `idls/zk_shielded_pool_solana.json` (`declare_program!`). After a program
  change run `just program-build` (anchor build in `../pool-anchor-v2-rewrite` + `scripts/sync-idl.sh`).

### Privacy limit (MVP)

The server never signs a deposit and its withdrawal with the same wallet. But if a withdraw wallet is funded from a
deposit wallet, the two are linked on chain. Fund withdraw wallets from an unrelated source.

## Tests

- `cargo test`: unit tests and LiteSVM tests against `../pool-anchor-v2-rewrite/target/deploy/zk_shielded_pool_solana.so`
  (`ZK_POOL_SO` to override; skipped with a message if missing). No network.
- `just it`: starts Surfpool (mainnet fork, mainnet feature set), deploys the program with the `deployment` runbook,
  runs the Rust integration tests (`tests/integration_tests/it_*.rs`), and stops Surfpool.
  - `SURFPOOL_OFFLINE=1 just it`: no mainnet fork (fast, no network).
  - `SURFPOOL_FEATURES=<pubkey,...> just it`: extra feature gates. **Not a mainnet check.**
    The current program needs `sol_big_mod_exp`, which mainnet does not have active, so the deploy fails without
    `SURFPOOL_FEATURES=EBq48m8irRKuE7ZnMTLvLg2UuGSqhe8s8oMqnmja1fJw`.
    That gate only unblocks the deploy (initialize/deposit). The program targets the Agave 4.3 syscall
    (SIMD-0529, gate `expH2ppKPW2ANEdEmAjfhSEcnBQJfmoX4FjuNpe9ttg`, little-endian). Surfpool 1.5 runs the older
    big-endian version under `EBq48m…` and does not know `expH2pp…`, so **withdraw results on Surfpool are invalid**.
- `just it --nocapture`: prints test output.

- `just surfpool` + `just it-surfpool`: same tests against a surfnet you keep running.



## Test endpoints

- /health
```
curl -i localhost:3000/health
```

- /deposit: `deposit_wallet_address` picks the signer from `DEPOSIT_WALLET_PRIVATE_KEYS`. Chunks are lamports
  (a chunk may be 0); the total is their sum and must reach the pool minimum.
```
curl -i -X POST localhost:3000/deposit \
  -H 'content-type: application/json' \
  -d '{
    "chunks": [2000000000, 3000000000, 4000000000],
    "withdraw_addresses": [
      "<base58 recipient 1>",
      "<base58 recipient 2>",
      "<base58 recipient 3>"
    ],
    "deposit_wallet_address": "<base58 deposit wallet>"
  }'
```
  The response holds `deposit_id`, `tx_signature`, `total_amount`, `new_root` (hex LE) and the full `zk_deposit_data`,
  including the secret `secret_s`. The same zk deposit data is stored in `ZK_DEPOSIT_DATA`. Keep it private: it is what
  withdraws the funds.
