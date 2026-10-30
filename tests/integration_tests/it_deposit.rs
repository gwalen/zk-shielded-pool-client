//! Surfpool integration tests for `POST /deposit`, run by `just it`.
//!
//! The surfnet is shared between tests and test binaries, so every test compares chain state
//! before and after its own request.

#[path = "../common/mod.rs"]
mod common;

use std::{array, fs, sync::Arc};

use axum::http::StatusCode;
use common::{
    admin_ixs,
    surfnet::{LAMPORTS_PER_SOL, SurfnetEnv, ZkPoolClientTestServer},
};
use serde_json::{Value, json};
use shielded_pool_circuit::circuit::{
    full_circuit::{deposit_commitment_hash, user_commitment_hash},
    utils::{convert_pubkey_32bytes_to_fr, fr_to_le_bytes},
};
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use zk_shielded_pool_client::{
    api::data::deposit_output::DepositOutput,
    solana::{
        accounts_helpers::{fetch_program_config, fetch_root_registry},
        program_helpers::derive_vault_pda,
        tx_builder::send_v0,
    },
    zk::{
        field::Fr,
        zk_deposit_data::{ZkDepositData, ZkDepositStatus},
    },
};

fn deposit_body(server: &ZkPoolClientTestServer, chunks: [u64; 3]) -> Value {
    let withdraw_addresses: [String; 3] = array::from_fn(|_| Pubkey::new_unique().to_string());
    json!({
        "chunks": chunks,
        "withdraw_addresses": withdraw_addresses,
        "deposit_wallet_address": deposit_wallet(server).pubkey().to_string(),
    })
}

fn deposit_wallet(server: &ZkPoolClientTestServer) -> Arc<Keypair> {
    server.state.deposit_wallets.default()
}

/// Every zk deposit data file the server stored.
fn read_stored_zk_deposit_data(server: &ZkPoolClientTestServer) -> Vec<ZkDepositData> {
    fs::read_dir(&server.state.config.zk_deposit_data_dir)
        .unwrap()
        .map(|entry| serde_json::from_slice(&fs::read(entry.unwrap().path()).unwrap()).unwrap())
        .collect()
}

/// Pretty-prints every stored zk deposit data (`secret_s` redacted). Shown with `just it --nocapture`.
fn print_stored_zk_deposit_data(server: &ZkPoolClientTestServer) {
    for zk_deposit_data in read_stored_zk_deposit_data(server) {
        println!("{zk_deposit_data:#?}");
    }
}

/// `deposit_commitment_hash` computed again from the zk deposit data's own secret, chunks and destinations.
fn recompute_deposit_commitment_hash(zk_deposit_data: &ZkDepositData) -> String {
    let destinations = zk_deposit_data.destinations.each_ref().map(|destination| {
        convert_pubkey_32bytes_to_fr(destination.parse::<Pubkey>().unwrap().to_bytes())
    });
    let user_commitment_hash = user_commitment_hash(
        zk_deposit_data.secret().unwrap(),
        &zk_deposit_data.chunks.map(Fr::from),
        &destinations,
    );
    hex::encode(fr_to_le_bytes(deposit_commitment_hash(
        user_commitment_hash,
        Fr::from(zk_deposit_data.total_amount),
    )))
}

#[tokio::test]
#[ignore = "needs surfpool: run `just it`"]
async fn deposit_happy_path() {
    let env = SurfnetEnv::connect().await;
    env.ensure_pool_initialized().await;
    let server = env.start_server().await;
    let depositor = deposit_wallet(&server).pubkey();
    let chunks = [2, 3, 4].map(|sol| sol * LAMPORTS_PER_SOL);
    let total: u64 = chunks.iter().sum();

    let next_leaf_idx_before = fetch_root_registry(&env.rpc)
        .await
        .unwrap()
        .unwrap()
        .next_leaf_idx;
    let vault_balance_before = env.rpc.get_balance(&derive_vault_pda()).await.unwrap();
    let depositor_balance_before = env.rpc.get_balance(&depositor).await.unwrap();

    let (status, body) = server
        .post_json("/deposit", deposit_body(&server, chunks))
        .await;

    print_stored_zk_deposit_data(&server);

    assert_eq!(status, StatusCode::OK, "{body}");
    let output: DepositOutput = serde_json::from_value(body).unwrap();

    let root_registry_account_view = fetch_root_registry(&env.rpc).await.unwrap().unwrap();
    assert_eq!(
        root_registry_account_view.next_leaf_idx,
        next_leaf_idx_before + 1
    );
    assert_eq!(
        hex::encode(root_registry_account_view.root_le),
        output.new_root
    );
    assert!(root_registry_account_view.is_known_root(&root_registry_account_view.root_le));
    assert_eq!(
        env.rpc.get_balance(&derive_vault_pda()).await.unwrap(),
        vault_balance_before + total
    );
    assert!(env.rpc.get_balance(&depositor).await.unwrap() <= depositor_balance_before - total);

    assert_eq!(output.total_amount, total);
    assert_eq!(output.deposit_id, output.zk_deposit_data.id);

    let stored_zk_deposit_data = server
        .state
        .zk_deposit_data_writer
        .get(&output.deposit_id)
        .await
        .unwrap();
    assert_eq!(
        stored_zk_deposit_data, output.zk_deposit_data,
        "the response returns the stored zk deposit data"
    );
    assert_eq!(stored_zk_deposit_data.status, ZkDepositStatus::Confirmed);
    assert_eq!(
        stored_zk_deposit_data.deposit_tx.as_deref(),
        Some(output.tx_signature.as_str())
    );
    assert_eq!(stored_zk_deposit_data.deposit_wallet, depositor.to_string());
    assert_eq!(
        recompute_deposit_commitment_hash(&stored_zk_deposit_data),
        stored_zk_deposit_data.deposit_commitment_hash
    );
}

#[tokio::test]
#[ignore = "needs surfpool: run `just it`"]
async fn deposit_with_zero_chunk() {
    let env = SurfnetEnv::connect().await;
    env.ensure_pool_initialized().await;
    let server = env.start_server().await;

    let chunks = [LAMPORTS_PER_SOL, 0, 2 * LAMPORTS_PER_SOL];
    let (status, body) = server
        .post_json("/deposit", deposit_body(&server, chunks))
        .await;

    print_stored_zk_deposit_data(&server);

    assert_eq!(status, StatusCode::OK, "{body}");

    let output: DepositOutput = serde_json::from_value(body).unwrap();
    assert_eq!(output.zk_deposit_data.chunks, chunks);
    assert_eq!(output.zk_deposit_data.status, ZkDepositStatus::Confirmed);
}

#[tokio::test]
#[ignore = "needs surfpool: run `just it`"]
async fn deposit_below_minimum() {
    let env = SurfnetEnv::connect().await;
    env.ensure_pool_initialized().await;
    let server = env.start_server().await;
    let min_deposit_lamports = fetch_program_config(&env.rpc)
        .await
        .unwrap()
        .unwrap()
        .min_deposit_lamports;

    let next_leaf_idx_before = fetch_root_registry(&env.rpc)
        .await
        .unwrap()
        .unwrap()
        .next_leaf_idx;

    let (status, body) = server
        .post_json(
            "/deposit",
            deposit_body(&server, [min_deposit_lamports - 1, 0, 0]),
        )
        .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("below the pool minimum"),
        "{body}"
    );
    assert_eq!(
        fetch_root_registry(&env.rpc)
            .await
            .unwrap()
            .unwrap()
            .next_leaf_idx,
        next_leaf_idx_before
    );
    assert!(read_stored_zk_deposit_data(&server).is_empty());
}

#[tokio::test]
#[ignore = "needs surfpool: run `just it`"]
async fn deposit_program_paused() {
    let env = SurfnetEnv::connect().await;
    let owner = env.ensure_pool_initialized().await;
    let server = env.start_server().await;
    let next_leaf_idx_before = fetch_root_registry(&env.rpc)
        .await
        .unwrap()
        .unwrap()
        .next_leaf_idx;

    send_v0(
        &env.rpc,
        &owner,
        &[admin_ixs::pause(&env.pool, owner.pubkey())],
    )
    .await
    .expect("pause");

    let (status, body) = server
        .post_json("/deposit", deposit_body(&server, [LAMPORTS_PER_SOL, 0, 0]))
        .await;

    let next_leaf_idx_after = fetch_root_registry(&env.rpc)
        .await
        .unwrap()
        .unwrap()
        .next_leaf_idx;

    // Unpause before asserting, so a failed assertion leaves the shared pool usable.
    send_v0(
        &env.rpc,
        &owner,
        &[admin_ixs::unpause(&env.pool, owner.pubkey())],
    )
    .await
    .expect("unpause");

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"].as_str().unwrap().contains("paused"), "{body}");
    assert_eq!(next_leaf_idx_after, next_leaf_idx_before);
    assert!(read_stored_zk_deposit_data(&server).is_empty());
}

#[tokio::test]
#[ignore = "needs surfpool: run `just it`"]
async fn deposit_unknown_or_withdraw_wallet() {
    let env = SurfnetEnv::connect().await;
    env.ensure_pool_initialized().await;
    let server = env.start_server().await;
    let next_leaf_idx_before = fetch_root_registry(&env.rpc)
        .await
        .unwrap()
        .unwrap()
        .next_leaf_idx;
    let withdraw_wallet = server.state.withdraw_wallets.default().pubkey();

    for wallet in [withdraw_wallet, Pubkey::new_unique()] {
        let mut body = deposit_body(&server, [LAMPORTS_PER_SOL, 0, 0]);
        body["deposit_wallet_address"] = json!(wallet.to_string());
        let (status, body) = server.post_json("/deposit", body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("not a configured deposit wallet"),
            "{body}"
        );
    }

    let next_leaf_idx_after = fetch_root_registry(&env.rpc)
        .await
        .unwrap()
        .unwrap()
        .next_leaf_idx;
    assert_eq!(next_leaf_idx_after, next_leaf_idx_before);
    assert!(read_stored_zk_deposit_data(&server).is_empty());
}

#[tokio::test]
#[ignore = "needs surfpool: run `just it`"]
async fn deposit_insufficient_funds() {
    let env = SurfnetEnv::connect().await;
    env.ensure_pool_initialized().await;
    let server = env.start_server().await;
    let deposit_wallet_balance = env
        .rpc
        .get_balance(&deposit_wallet(&server).pubkey())
        .await
        .unwrap();
    let next_leaf_idx_before = fetch_root_registry(&env.rpc)
        .await
        .unwrap()
        .unwrap()
        .next_leaf_idx;

    // One SOL more than the deposit wallet holds.
    let (status, body) = server
        .post_json(
            "/deposit",
            deposit_body(&server, [deposit_wallet_balance, LAMPORTS_PER_SOL, 0]),
        )
        .await;
    print_stored_zk_deposit_data(&server);

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    let next_leaf_idx_after = fetch_root_registry(&env.rpc)
        .await
        .unwrap()
        .unwrap()
        .next_leaf_idx;
    assert_eq!(next_leaf_idx_after, next_leaf_idx_before);

    let [zk_deposit_data] = &read_stored_zk_deposit_data(&server)[..] else {
        panic!("expected one stored zk deposit data file")
    };
    assert_eq!(zk_deposit_data.status, ZkDepositStatus::Failed);
    assert_eq!(zk_deposit_data.deposit_tx, None);
    let error = zk_deposit_data
        .error
        .as_deref()
        .expect("the zk deposit data records the failure");
    assert!(error.contains("instruction 0 failed"), "{error}");
}

#[tokio::test]
#[ignore = "needs surfpool: run `just it`"]
async fn two_deposits_same_slot() {
    let env = SurfnetEnv::connect().await;
    env.ensure_pool_initialized().await;
    let server = env.start_server().await;
    let next_leaf_idx_before = fetch_root_registry(&env.rpc)
        .await
        .unwrap()
        .unwrap()
        .next_leaf_idx;

    let ((first_status, first), (second_status, second)) = tokio::join!(
        server.post_json("/deposit", deposit_body(&server, [LAMPORTS_PER_SOL, 0, 0])),
        server.post_json(
            "/deposit",
            deposit_body(&server, [2 * LAMPORTS_PER_SOL, 0, 0])
        ),
    );
    print_stored_zk_deposit_data(&server);

    assert_eq!(first_status, StatusCode::OK, "{first}");
    assert_eq!(second_status, StatusCode::OK, "{second}");

    let root_registry_account_view = fetch_root_registry(&env.rpc).await.unwrap().unwrap();
    assert_eq!(
        root_registry_account_view.next_leaf_idx,
        next_leaf_idx_before + 2
    );

    for body in [first, second] {
        let output: DepositOutput = serde_json::from_value(body).unwrap();
        assert_eq!(output.zk_deposit_data.status, ZkDepositStatus::Confirmed);
        assert!(
            root_registry_account_view
                .is_known_root(&hex::decode(&output.new_root).unwrap().try_into().unwrap())
        );
    }
}
