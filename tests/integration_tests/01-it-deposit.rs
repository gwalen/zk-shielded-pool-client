//! Surfpool integration tests for `POST /deposit`, run by `just it`.
//!
//! The surfnet is shared between tests and test binaries, so every test compares chain state
//! before and after its own request.

#[path = "../common/mod.rs"]
mod common;

use std::{array, fs, sync::Arc};

use axum::http::StatusCode;
use chrono::Utc;
use common::{
    admin_ixs,
    mock_failing_rpc_sender::SIMULATED_RPC_FAILURE,
    surfnet::{LAMPORTS_PER_SOL, SurfnetEnv, ZkPoolClientTestServer},
};
use serde_json::{Value, json};
use shielded_pool_circuit::circuit::{
    full_circuit::{deposit_commitment_hash, user_commitment_hash},
    utils::{convert_pubkey_32bytes_to_fr, fr_to_le_bytes},
};
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_rpc_client_api::{config::RpcSendTransactionConfig, request::RpcRequest};
use solana_signature::Signature;
use solana_signer::Signer;
use zk_shielded_pool_client::{
    api::data::{deposit_output::DepositOutput, reconcile_deposit_output::ReconcileDepositOutput},
    solana::{
        accounts_helpers::{fetch_program_config, fetch_root_registry},
        program_helpers::derive_vault_pda,
        tx_builder::{compile_v0, send_v0},
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

/// Stored deposit outcome with a fresh id, for the reconcile tests.
fn create_stored_zk_deposit_data(
    status: ZkDepositStatus,
    tx_signature: &Signature,
) -> ZkDepositData {
    let id = hex::encode(Pubkey::new_unique().to_bytes());
    ZkDepositData {
        id: id.clone(),
        secret_s: hex::encode(fr_to_le_bytes(Fr::from(1u64))),
        deposit_wallet: Pubkey::new_unique().to_string(),
        total_amount: LAMPORTS_PER_SOL,
        chunks: [LAMPORTS_PER_SOL, 0, 0],
        destinations: array::from_fn(|_| Pubkey::new_unique().to_string()),
        user_commitment_hash: hex::encode([1; 32]),
        deposit_commitment_hash: id,
        status,
        deposit_tx: tx_signature.to_string(),
        error: None,
        created_at: Utc::now(),
        withdrawals: [None, None, None],
    }
}

/// The 500 body of a deposit whose tx may have landed names the stored deposit and its tx.
fn assert_pending_outcome_body(body: &Value, zk_deposit_data: &ZkDepositData) {
    assert_eq!(body["deposit_id"], zk_deposit_data.id.as_str(), "{body}");
    assert_eq!(
        body["tx_signature"],
        zk_deposit_data.deposit_tx.as_str(),
        "{body}"
    );
    assert_eq!(body["status"], "pending", "{body}");
}

async fn post_reconcile(
    server: &ZkPoolClientTestServer,
    deposit_id: &str,
) -> ReconcileDepositOutput {
    let (status, body) = server
        .post_json(&format!("/deposit/{deposit_id}/reconcile"), json!({}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

#[tokio::test]
#[ignore = "needs surfpool: run `just it`"]
async fn deposit_happy_path() {
    let env = SurfnetEnv::connect().await;
    env.ensure_pool_initialized().await;
    let server = env.start_server("deposit_happy_path").await;
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
    assert_eq!(stored_zk_deposit_data.deposit_tx, output.tx_signature);
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
    let server = env.start_server("deposit_with_zero_chunk").await;

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
    let server = env.start_server("deposit_below_minimum").await;
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
    let server = env.start_server("deposit_program_paused").await;
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
    let server = env.start_server("deposit_unknown_or_withdraw_wallet").await;
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
    let server = env.start_server("deposit_insufficient_funds").await;
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

    assert_eq!(body["error"], "transaction rejected", "{body}");
    let [zk_deposit_data] = &read_stored_zk_deposit_data(&server)[..] else {
        panic!("expected one stored zk deposit data file")
    };
    assert_eq!(zk_deposit_data.status, ZkDepositStatus::Failed);
    let error = zk_deposit_data
        .error
        .as_deref()
        .expect("the zk deposit data records the failure");
    assert!(error.contains("instruction 0 failed"), "{error}");
}

/// The tx is really sent, then every status poll fails: the deposit landed, so it must not be `Failed`.
#[tokio::test]
#[ignore = "needs surfpool: run `just it`"]
async fn deposit_confirmation_rpc_error_stays_pending_with_tx() {
    let env = SurfnetEnv::connect().await;
    env.ensure_pool_initialized().await;

    let server = env
        .start_server_with_rpc(
            "deposit_confirmation_rpc_error_stays_pending_with_tx",
            env.create_mock_failing_rpc(RpcRequest::GetSignatureStatuses, usize::MAX),
        )
        .await;

    let next_leaf_idx_before = fetch_root_registry(&env.rpc)
        .await
        .unwrap()
        .unwrap()
        .next_leaf_idx;

    let (status, body) = server
        .post_json("/deposit", deposit_body(&server, [LAMPORTS_PER_SOL, 0, 0]))
        .await;

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");

    let [zk_deposit_data] = &read_stored_zk_deposit_data(&server)[..] else {
        panic!("expected one stored zk deposit data file")
    };

    assert_eq!(zk_deposit_data.status, ZkDepositStatus::Pending);
    assert_pending_outcome_body(&body, zk_deposit_data);

    let error = zk_deposit_data
        .error
        .as_deref()
        .expect("the zk deposit data records the send error");

    assert!(error.contains(SIMULATED_RPC_FAILURE), "{error}");

    let tx_signature: Signature = zk_deposit_data.deposit_tx.parse().unwrap();

    env.rpc.poll_for_signature(&tx_signature).await.unwrap();

    let next_leaf_idx_after = fetch_root_registry(&env.rpc)
        .await
        .unwrap()
        .unwrap()
        .next_leaf_idx;

    assert_eq!(next_leaf_idx_after, next_leaf_idx_before + 1);
}

/// A brief `get_transaction` failure after the tx is confirmed must not leave the deposit `Pending`.
#[tokio::test]
#[ignore = "needs surfpool: run `just it`"]
async fn deposit_retries_get_transaction_and_succeeds() {
    let env = SurfnetEnv::connect().await;
    env.ensure_pool_initialized().await;

    let server = env
        .start_server_with_rpc(
            "deposit_retries_get_transaction_and_succeeds",
            env.create_mock_failing_rpc(RpcRequest::GetTransaction, 2),
        )
        .await;

    let (status, body) = server
        .post_json("/deposit", deposit_body(&server, [LAMPORTS_PER_SOL, 0, 0]))
        .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let output: DepositOutput = serde_json::from_value(body).unwrap();
    assert_eq!(output.zk_deposit_data.status, ZkDepositStatus::Confirmed);
}

/// The retry is bounded: a lasting `get_transaction` failure returns an error and keeps the tx.
#[tokio::test]
#[ignore = "needs surfpool: run `just it`"]
async fn deposit_retries_get_transaction_and_fails() {
    let env = SurfnetEnv::connect().await;
    env.ensure_pool_initialized().await;

    let server = env
        .start_server_with_rpc(
            "deposit_retries_get_transaction_and_fails",
            env.create_mock_failing_rpc(RpcRequest::GetTransaction, usize::MAX),
        )
        .await;

    let (status, body) = server
        .post_json("/deposit", deposit_body(&server, [LAMPORTS_PER_SOL, 0, 0]))
        .await;

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    let [zk_deposit_data] = &read_stored_zk_deposit_data(&server)[..] else {
        panic!("expected one stored zk deposit data file")
    };
    assert_eq!(zk_deposit_data.status, ZkDepositStatus::Pending);
    assert_pending_outcome_body(&body, zk_deposit_data);
    let error = zk_deposit_data
        .error
        .as_deref()
        .expect("the zk deposit data records the log fetch error");
    assert!(error.contains(SIMULATED_RPC_FAILURE), "{error}");
}

#[tokio::test]
#[ignore = "needs surfpool: run `just it`"]
async fn two_deposits_same_slot() {
    let env = SurfnetEnv::connect().await;
    env.ensure_pool_initialized().await;
    let server = env.start_server("two_deposits_same_slot").await;
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

/// One failed status poll loses the send outcome. The tx landed, so reconcile confirms the deposit.
#[tokio::test]
#[ignore = "needs surfpool: run `just it`"]
async fn reconcile_confirms_a_landed_deposit() {
    let env = SurfnetEnv::connect().await;
    env.ensure_pool_initialized().await;

    let server = env
        .start_server_with_rpc(
            "reconcile_confirms_a_landed_deposit",
            env.create_mock_failing_rpc(RpcRequest::GetSignatureStatuses, 1),
        )
        .await;

    let (status, body) = server
        .post_json("/deposit", deposit_body(&server, [LAMPORTS_PER_SOL, 0, 0]))
        .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");

    let [zk_deposit_data] = &read_stored_zk_deposit_data(&server)[..] else {
        panic!("expected one stored zk deposit data file")
    };
    assert_eq!(zk_deposit_data.status, ZkDepositStatus::Pending);
    assert_pending_outcome_body(&body, zk_deposit_data);

    let tx_signature: Signature = zk_deposit_data.deposit_tx.parse().unwrap();
    env.rpc.poll_for_signature(&tx_signature).await.unwrap();

    let output = post_reconcile(&server, &zk_deposit_data.id).await;

    assert_eq!(output.deposit_id, zk_deposit_data.id);
    assert_eq!(output.status, ZkDepositStatus::Confirmed);
    assert_eq!(output.tx_signature, tx_signature.to_string());
    assert_eq!(output.error, None);

    let stored_zk_deposit_data = server
        .state
        .zk_deposit_data_writer
        .get(&zk_deposit_data.id)
        .await
        .unwrap();
    assert_eq!(stored_zk_deposit_data.status, ZkDepositStatus::Confirmed);
    assert_eq!(stored_zk_deposit_data.error, None);
}

#[tokio::test]
#[ignore = "needs surfpool: run `just it`"]
async fn reconcile_keeps_pending_when_the_tx_never_landed() {
    let env = SurfnetEnv::connect().await;
    env.ensure_pool_initialized().await;

    let server = env
        .start_server("reconcile_keeps_pending_when_the_tx_never_landed")
        .await;

    let zk_deposit_data =
        create_stored_zk_deposit_data(ZkDepositStatus::Pending, &Signature::from([7; 64]));
    server
        .state
        .zk_deposit_data_writer
        .create(&zk_deposit_data)
        .await
        .unwrap();

    let output = post_reconcile(&server, &zk_deposit_data.id).await;

    assert_eq!(output.status, ZkDepositStatus::Pending);
    assert_eq!(
        server
            .state
            .zk_deposit_data_writer
            .get(&zk_deposit_data.id)
            .await
            .unwrap(),
        zk_deposit_data
    );
}

/// The tx lands but fails, so reconcile marks the deposit `Failed` and records why.
#[tokio::test]
#[ignore = "needs surfpool: run `just it`"]
async fn reconcile_fails_a_deposit_whose_tx_failed_on_chain() {
    let env = SurfnetEnv::connect().await;
    env.ensure_pool_initialized().await;

    let server = env
        .start_server("reconcile_fails_a_deposit_whose_tx_failed_on_chain")
        .await;

    let depositor = env.create_funded_keypair(LAMPORTS_PER_SOL).await;

    let deposit_ix = env.pool.deposit(
        depositor.pubkey(),
        fr_to_le_bytes(Fr::from(1u64)),
        2 * LAMPORTS_PER_SOL,
    );
    let blockhash = env.rpc.get_latest_blockhash().await.unwrap();
    let deposit_tx = compile_v0(&depositor, &[deposit_ix], blockhash).unwrap();

    let send_transaction_config = RpcSendTransactionConfig {
        skip_preflight: true,
        ..RpcSendTransactionConfig::default()
    };

    let tx_signature = env
        .rpc
        .send_transaction_with_config(&deposit_tx, send_transaction_config)
        .await
        .unwrap();
    env.rpc.poll_for_signature(&tx_signature).await.unwrap();

    let zk_deposit_data = create_stored_zk_deposit_data(ZkDepositStatus::Pending, &tx_signature);
    server
        .state
        .zk_deposit_data_writer
        .create(&zk_deposit_data)
        .await
        .unwrap();

    let output = post_reconcile(&server, &zk_deposit_data.id).await;

    assert_eq!(output.status, ZkDepositStatus::Failed);
    let error = output.error.expect("reconcile records the failure");
    assert!(error.contains("Transaction rejected"), "{error}");
}

/// The tx landed but its event differs from the stored commitment, so the deposit stays `Pending`.
#[tokio::test]
#[ignore = "needs surfpool: run `just it`"]
async fn reconcile_keeps_pending_when_the_event_check_fails() {
    let env = SurfnetEnv::connect().await;
    env.ensure_pool_initialized().await;

    let server = env
        .start_server("reconcile_keeps_pending_when_the_event_check_fails")
        .await;

    let (status, body) = server
        .post_json("/deposit", deposit_body(&server, [LAMPORTS_PER_SOL, 0, 0]))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let deposit_output: DepositOutput = serde_json::from_value(body).unwrap();

    let other_commitment_zk_deposit_data = create_stored_zk_deposit_data(
        ZkDepositStatus::Pending,
        &deposit_output.tx_signature.parse().unwrap(),
    );
    server
        .state
        .zk_deposit_data_writer
        .create(&other_commitment_zk_deposit_data)
        .await
        .unwrap();

    let output = post_reconcile(&server, &other_commitment_zk_deposit_data.id).await;

    assert_eq!(output.status, ZkDepositStatus::Pending);
    let error = output.error.expect("reconcile records the failed check");
    assert!(
        error.contains("differs from the local commitment"),
        "{error}"
    );
}

/// Every `get_transaction` call fails on this server, so a passing request proves no RPC call was made.
#[tokio::test]
#[ignore = "needs surfpool: run `just it`"]
async fn reconcile_returns_final_status_without_rpc() {
    let env = SurfnetEnv::connect().await;
    env.ensure_pool_initialized().await;

    let server = env
        .start_server_with_rpc(
            "reconcile_returns_final_status_without_rpc",
            env.create_mock_failing_rpc(RpcRequest::GetTransaction, usize::MAX),
        )
        .await;

    for final_status in [ZkDepositStatus::Confirmed, ZkDepositStatus::Failed] {
        let zk_deposit_data =
            create_stored_zk_deposit_data(final_status, &Signature::from([7; 64]));
        server
            .state
            .zk_deposit_data_writer
            .create(&zk_deposit_data)
            .await
            .unwrap();

        let output = post_reconcile(&server, &zk_deposit_data.id).await;

        assert_eq!(output.status, final_status);
        assert_eq!(output.tx_signature, zk_deposit_data.deposit_tx);
        assert_eq!(
            server
                .state
                .zk_deposit_data_writer
                .get(&zk_deposit_data.id)
                .await
                .unwrap(),
            zk_deposit_data
        );
    }
}

#[tokio::test]
#[ignore = "needs surfpool: run `just it`"]
async fn reconcile_unknown_deposit_id_is_not_found() {
    let env = SurfnetEnv::connect().await;
    env.ensure_pool_initialized().await;

    let server = env
        .start_server("reconcile_unknown_deposit_id_is_not_found")
        .await;

    let unknown_deposit_id = hex::encode(Pubkey::new_unique().to_bytes());
    let (status, body) = server
        .post_json(
            &format!("/deposit/{unknown_deposit_id}/reconcile"),
            json!({}),
        )
        .await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}
