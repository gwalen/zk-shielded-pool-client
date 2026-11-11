# Client for zk-shielded-pool contracts

Contains the client code for ZK-shielded-pool. It is a Rust backend server app for interacting with the Solana program: sending deposit/withdrawal transactions and building proofs for the verifier. For educational purposes, it must be run locally with a private key set in an environment variable. In future versions, this will be replaced by a web app with WASM code.

This is the client for the broader [zk-shielded-pool](https://github.com/gwalen/zk-shielded-pool) project.

## Setup

- Copy `.env.example` to `.env` and fill in the wallet keys.
  - `DEPOSIT_WALLET_PRIVATE_KEYS`: wallets that sign deposits and pay the funds and fees.
  - `WITHDRAW_WALLET_PRIVATE_KEYS`: fee payers and signers for `upload_proof`, `withdraw` and `close_proof`.
    The first one is the default.
  - A key may not be in both lists (startup fails). Logs show public keys only.
- Plain `cargo build` works from a clean checkout. 
- The program interface is generated from `idls/zk_shielded_pool_solana.json` (`declare_program!`). After a program
  change run `just program-build` (anchor build in `../pool-anchor-v2-rewrite` + `scripts/sync-idl.sh`).
- Run `scripts/fetch-srs.sh` once. It downloads the KZG params (8 MB, not committed) to
  `resources/kzg_bn254_16.srs`, the `SRS_PATH` the withdraw prover loads. At startup the server runs keygen (a few
  seconds) and stops if the circuit VK differs from `resources/vk.bin`, the VK the program embeds. After a
  `cargo update` that moves `shielded-pool-circuit`, copy `vk.bin` and `kzg_vk.bin` from that solana-proof-generator
  commit into `resources/` and redeploy the program with the same VK.

## Tests

```sh
just it
```

```sh
cargo test
```

- `cargo test`: unit tests and LiteSVM tests against `../pool-anchor-v2-rewrite/target/deploy/zk_shielded_pool_solana.so`
  (`ZK_POOL_SO` to override; skipped with a message if missing). No network. The Surfpool tests show as ignored here.
- `cargo test -- --ignored`: also the slow prover tests (keygen, proofs of the fixture steps, a LiteSVM withdraw).
  They need `resources/kzg_bn254_16.srs`. With `--ignored` alone the Surfpool tests run too and need `just it`.
- `just it`: starts its own Surfpool, so no node has to be running. 
  - By default it runs offline, with no mainnet fork, and turns on the `sol_big_mod_exp` feature the withdraw verifier
    needs (SIMD-0529, gate `expH2ppKPW2ANEdEmAjfhSEcnBQJfmoX4FjuNpe9ttg`). The feature isn't active on mainnet or
    devnet, yet. Surfpool 1.7.0 implements this syscall, so withdraw tests belong on
    Surfpool too. The defaults are at the top of the `justfile`.
- `just it --nocapture`: prints test output.
- `just surfpool` + `just it-surfpool`: same tests against a surfnet you keep running.


## Test endpoints

- /health
```
curl -i localhost:3000/health
```
- /stats
```
curl -i localhost:3000/stats
```
