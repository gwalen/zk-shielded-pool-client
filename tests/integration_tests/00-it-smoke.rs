//! Surfpool smoke test, run first by `just it`: the pool is initialized and the generated
//! account types decode an account deployed the mainnet way (runbook deploy, mainnet features).

#[path = "../common/mod.rs"]
mod common;

use std::{fs, path::Path};

use axum::http::StatusCode;
use common::surfnet::{IT_ZK_DEPOSIT_DATA_DIR, SurfnetEnv};
use shielded_pool_circuit::circuit::utils::fr_to_le_bytes;
use solana_signer::Signer;
use zk_shielded_pool_client::{
    solana::accounts_helpers::{fetch_program_config, fetch_root_registry},
    zk::field::create_empty_tree_root,
};

#[tokio::test]
#[ignore = "needs surfpool: run `just it`"]
async fn smoke_pool_initialized() {
    // Runs first, so each run starts with an empty zk deposit data folder.
    if Path::new(IT_ZK_DEPOSIT_DATA_DIR).exists() {
        fs::remove_dir_all(IT_ZK_DEPOSIT_DATA_DIR).unwrap();
    }

    let env = SurfnetEnv::connect().await;
    let owner = env.ensure_pool_initialized().await;

    let program_config_account_view = fetch_program_config(&env.rpc)
        .await
        .unwrap()
        .expect("program_config exists");
    let root_registry_account_view = fetch_root_registry(&env.rpc)
        .await
        .unwrap()
        .expect("root_registry exists");
    assert_eq!(program_config_account_view.owner, owner.pubkey());
    assert!(!program_config_account_view.paused);
    assert!(program_config_account_view.min_deposit_lamports > 0);

    if root_registry_account_view.next_leaf_idx == 0 {
        // Fresh surfnet: the depth-20 empty root.
        assert_eq!(
            root_registry_account_view.root_le,
            fr_to_le_bytes(create_empty_tree_root())
        );
    } else {
        eprintln!(
            "surfnet is not fresh: {} leaves already",
            root_registry_account_view.next_leaf_idx
        );
    }
    assert!(root_registry_account_view.is_known_root(&root_registry_account_view.root_le));

    // The real server, on a local port, against the surfnet.
    let server = env.start_server("smoke_pool_initialized").await;
    let (status, body) = server.get("/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["msg"], "OK");
}
