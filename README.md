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
- Run `scripts/fetch-srs.sh` once. It downloads the KZG params (8 MB, not committed) to
  `resources/kzg_bn254_16.srs`, the `SRS_PATH` the withdraw prover loads. At startup the server runs keygen (a few
  seconds) and stops if the circuit VK differs from `resources/vk.bin`, the VK the program embeds. After a
  `cargo update` that moves `shielded-pool-circuit`, copy `vk.bin` and `kzg_vk.bin` from that solana-proof-generator
  commit into `resources/` and redeploy the program with the same VK.

### Privacy limit (MVP)

The server never signs a deposit and its withdrawal with the same wallet. But if a withdraw wallet is funded from a
deposit wallet, the two are linked on chain. Fund withdraw wallets from an unrelated source.

## Tests

A change isn't checked until both `cargo test` and `just it` pass.

- `cargo test`: unit tests and LiteSVM tests against `../pool-anchor-v2-rewrite/target/deploy/zk_shielded_pool_solana.so`
  (`ZK_POOL_SO` to override; skipped with a message if missing). No network. The Surfpool tests show as ignored here.
- `cargo test -- --ignored`: also the slow prover tests (keygen, proofs of the fixture steps, a LiteSVM withdraw).
  They need `resources/kzg_bn254_16.srs`. With `--ignored` alone the Surfpool tests run too and need `just it`.
- `just it`: starts its own Surfpool, so no node has to be running. It deploys the program with the `deployment`
  runbook, runs the Rust integration tests (`tests/integration_tests/*-it-*.rs`), and stops Surfpool. It needs
  Surfpool >= 1.7.0 and the built program in `../pool-anchor-v2-rewrite`. Each test keeps its zk deposit data in
  `tests/tmp/data/<test name>`, emptied when that test starts. The smoke test empties `tests/tmp/data`. The test
  servers of each test binary share one prover, so `just it` also needs `resources/kzg_bn254_16.srs`.
  - By default it runs offline, with no mainnet fork, and turns on the `sol_big_mod_exp` feature the withdraw verifier
    needs (SIMD-0529, gate `expH2ppKPW2ANEdEmAjfhSEcnBQJfmoX4FjuNpe9ttg`). The feature isn't active on mainnet or
    devnet, so this is **not a mainnet check**. Surfpool 1.7.0 implements this syscall, so withdraw tests belong on
    Surfpool too. The defaults are at the top of the `justfile`.
  - `SURFPOOL_OFFLINE= just it`: forks mainnet instead of running offline. Needs network.
  - `SURFPOOL_FEATURES=<pubkey,...> just it`: replaces the default feature gates. Without `expH2pp…` the deploy fails.
- `just it --nocapture`: prints test output.
- `just surfpool` + `just it-surfpool`: same tests against a surfnet you keep running.



## Test endpoints

- /health
```
curl -i localhost:3000/health
```

- /deposit: `deposit_wallet_address` picks the signer from `DEPOSIT_WALLET_PRIVATE_KEYS`. Chunks are lamports; the
  total is their sum and must reach the pool minimum. Each chunk is 0 or at least 890,880 lamports (the rent-exempt
  minimum of an empty account), otherwise the request gets a 400 that names the chunk.
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

  If the tx was sent but its outcome is unknown, the response is a 500 with `error`, `deposit_id`, `tx_signature` and
  `status: "pending"`. The tx may have landed, so check it with the reconcile endpoint before you retry.

- /deposit/{deposit_id}/reconcile: checks a `pending` deposit tx on chain. `confirmed` and `failed` are final and come
  back as stored. The response holds `deposit_id`, `status`, `tx_signature` and `error`. An unknown id gets a 404.
```
curl -i -X POST localhost:3000/deposit/<deposit_id>/reconcile
```

- /withdraw: proves and pays out one chunk (`step` 0, 1 or 2) of a `confirmed` deposit to its recipient.
  `fee_payer_address` picks the signer from `WITHDRAW_WALLET_PRIVATE_KEYS`; without it the first withdraw wallet pays.
  A `pending` or `failed` deposit gets a 409 that points to the reconcile endpoint. A step that was already withdrawn
  gets a 409, a zero chunk a 400. Proving takes several seconds and runs one proof at a time.
```
curl -i -X POST localhost:3000/withdraw \
  -H 'content-type: application/json' \
  -d '{ "deposit_id": "<deposit_id>", "step": 0 }'
```
  The response holds `withdraw_tx`, `upload_txs` (proof part 0 and 1), `close_proof_tx` (`null` if closing the proof
  account failed), `recipient`, `amount` (lamports) and `nullifier` (hex BE). The tree is rebuilt from the program's
  whole tx history on every withdraw, which suits a surfnet or devnet-sized history only.
