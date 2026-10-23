//! Surfpool smoke test, run first by `just it`: the pool is initialized and the generated
//! account types decode an account deployed the mainnet way (runbook deploy, mainnet features).

#[path = "../common/mod.rs"]
mod common;

use axum::http::StatusCode;
use solana_signer::Signer;
use common::surfnet::SurfnetEnv;
use zk_shielded_pool_client::{
    solana::accounts_helpers::{fetch_program_config, fetch_root_registry},
    zk::field::{empty_tree_root, fr_le},
};

#[tokio::test]
#[ignore = "needs surfpool: run `just it`"]
async fn smoke_pool_initialized() {
    let env = SurfnetEnv::connect().await;
    let owner = env.ensure_pool_initialized().await;

    let config = fetch_program_config(&env.rpc).await.unwrap().expect("program_config exists");
    let registry = fetch_root_registry(&env.rpc).await.unwrap().expect("root_registry exists");
    assert_eq!(config.owner, owner.pubkey());
    assert!(!config.paused);
    assert!(config.min_deposit_lamports > 0);

    if registry.next_leaf_idx == 0 {
        // Fresh surfnet: the depth-20 empty root.
        assert_eq!(registry.root_le, fr_le(empty_tree_root()));
    } else {
        eprintln!("surfnet is not fresh: {} leaves already", registry.next_leaf_idx);
    }
    assert!(registry.is_known_root(&registry.root_le));

    // The real server, on a local port, against the surfnet.
    let server = env.start_server().await;
    let (status, body) = server.get("/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["msg"], "OK");
}
