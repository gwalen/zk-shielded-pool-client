#[path = "../common/mod.rs"]
mod common;

use std::array;

use axum::http::StatusCode;
use common::surfnet::{LAMPORTS_PER_SOL, SurfnetEnv, ZkPoolClientTestServer};
use serde_json::{Value, json};
use shielded_pool_circuit::transcript::fr_to_be;
use solana_message::VersionedMessage;
use solana_pubkey::Pubkey;
use solana_rpc_client_api::config::RpcTransactionConfig;
use solana_signer::Signer;
use solana_transaction::versioned::VersionedTransaction;
use solana_transaction_status_client_types::UiTransactionEncoding;
use zk_shielded_pool_client::{
    api::data::{deposit_output::DepositOutput, withdraw_output::WithdrawOutput},
    solana::{
        constants::RPC_COMMITMENT,
        program_helpers::{ZK_SHIELDED_POOL_PROGRAM_ID, derive_nullifier_pda, derive_vault_pda},
    },
    zk::{
        field::{CHUNKS, create_nullifier},
        zk_deposit_data::ZkDepositStatus,
    },
};

/// Fee of one signature on the surfnet; the txs of a withdrawal set no priority fee.
const LAMPORTS_PER_SIGNATURE: u64 = 5_000;

/// Confirmed deposit through the server.
async fn post_deposit(
    server: &ZkPoolClientTestServer,
    chunks: [u64; CHUNKS],
    recipients: [Pubkey; CHUNKS],
) -> DepositOutput {
    let deposit_wallet_address = server.state.deposit_wallets.default().pubkey().to_string();

    let (status, body) = server
        .post_json(
            "/deposit",
            json!({
                "chunks": chunks,
                "withdraw_addresses": recipients.map(|recipient| recipient.to_string()),
                "deposit_wallet_address": deposit_wallet_address,
            }),
        )
        .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

/// Withdraw with the server's default withdraw wallet as fee payer.
fn create_withdraw_body(deposit_id: &str, step: usize) -> Value {
    json!({ "deposit_id": deposit_id, "step": step })
}

/// Successful withdraw through the server.
async fn post_withdraw(
    server: &ZkPoolClientTestServer,
    deposit_id: &str,
    step: usize,
) -> WithdrawOutput {
    let (status, body) = server
        .post_json("/withdraw", create_withdraw_body(deposit_id, step))
        .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn fetch_versioned_tx(env: &SurfnetEnv, tx_signature: &str) -> VersionedTransaction {
    let config = RpcTransactionConfig {
        encoding: Some(UiTransactionEncoding::Base64),
        commitment: Some(RPC_COMMITMENT),
        max_supported_transaction_version: Some(0),
    };

    env.rpc
        .get_transaction_with_config(&tx_signature.parse().unwrap(), config)
        .await
        .unwrap()
        .transaction
        .transaction
        .decode()
        .unwrap()
}

#[tokio::test]
#[ignore = "needs surfpool: run `just it`"]
async fn withdraw_all_steps() {
    let env = SurfnetEnv::connect().await;
    env.ensure_pool_initialized().await;
    let server = env.start_server("withdraw_all_steps").await;
    let recipients: [Pubkey; CHUNKS] = array::from_fn(|_| Pubkey::new_unique());
    let chunks = [1, 2, 3].map(|sol| sol * LAMPORTS_PER_SOL);

    let deposit_output = post_deposit(&server, chunks, recipients).await;
    let vault_balance_before = env.rpc.get_balance(&derive_vault_pda()).await.unwrap();

    let mut withdraw_outputs = Vec::new();
    for step in 0..CHUNKS {
        let withdraw_output = post_withdraw(&server, &deposit_output.deposit_id, step).await;

        assert_eq!(withdraw_output.recipient, recipients[step].to_string());
        assert_eq!(withdraw_output.amount, chunks[step]);
        assert_eq!(
            env.rpc.get_balance(&recipients[step]).await.unwrap(),
            chunks[step]
        );

        let nullifier_be: [u8; 32] = hex::decode(&withdraw_output.nullifier)
            .unwrap()
            .try_into()
            .unwrap();

        let nullifier_account = env
            .rpc
            .get_account_with_commitment(&derive_nullifier_pda(&nullifier_be), RPC_COMMITMENT)
            .await
            .unwrap()
            .value;

        assert!(
            nullifier_account.is_some(),
            "step {step}: nullifier account missing"
        );
        assert!(
            withdraw_output.close_proof_tx.is_some(),
            "step {step}: the proof account was not closed"
        );
        withdraw_outputs.push(withdraw_output);
    }

    assert_eq!(
        env.rpc.get_balance(&derive_vault_pda()).await.unwrap(),
        vault_balance_before - deposit_output.total_amount
    );

    let stored_zk_deposit_data = server
        .state
        .zk_deposit_data_writer
        .get(&deposit_output.deposit_id)
        .await
        .unwrap();

    for (withdrawal, withdraw_output) in stored_zk_deposit_data
        .withdrawals
        .iter()
        .zip(&withdraw_outputs)
    {
        let withdrawal = withdrawal.as_ref().expect("every step is recorded");
        assert_eq!(withdrawal.tx.as_ref(), Some(&withdraw_output.withdraw_tx));
        assert_eq!(withdrawal.nullifier_hex, withdraw_output.nullifier);
    }
}

/// Upload and withdraw txs are V0 with the compute budget; not a mainnet check (see `just it`).
#[tokio::test]
#[ignore = "needs surfpool: run `just it`"]
async fn withdraw_txs_are_v0() {
    let env = SurfnetEnv::connect().await;
    env.ensure_pool_initialized().await;
    let server = env.start_server("withdraw_txs_are_v0").await;
    let withdraw_wallet = server.state.withdraw_wallets.default().pubkey();

    let deposit_output = post_deposit(
        &server,
        [LAMPORTS_PER_SOL, 0, 0],
        array::from_fn(|_| Pubkey::new_unique()),
    )
    .await;

    let withdraw_output = post_withdraw(&server, &deposit_output.deposit_id, 0).await;

    for tx_signature in withdraw_output
        .upload_txs
        .iter()
        .chain([&withdraw_output.withdraw_tx])
    {
        let versioned_tx = fetch_versioned_tx(&env, tx_signature).await;
        assert!(
            matches!(versioned_tx.message, VersionedMessage::V0(_)),
            "{tx_signature} is not a V0 tx"
        );
        assert_eq!(
            versioned_tx.message.static_account_keys()[0],
            withdraw_wallet
        );
    }

    let withdraw_tx = fetch_versioned_tx(&env, &withdraw_output.withdraw_tx).await;

    let compute_budget_ix_count = withdraw_tx
        .message
        .instructions()
        .iter()
        .filter(|ix| {
            ix.program_id(withdraw_tx.message.static_account_keys())
                == &solana_compute_budget_interface::ID
        })
        .count();

    assert_eq!(compute_budget_ix_count, 2);
}

#[tokio::test]
#[ignore = "needs surfpool: run `just it`"]
async fn withdraw_twice() {
    let env = SurfnetEnv::connect().await;
    env.ensure_pool_initialized().await;
    let server = env.start_server("withdraw_twice").await;
    let withdraw_wallet = server.state.withdraw_wallets.default().pubkey();

    let deposit_output = post_deposit(
        &server,
        [LAMPORTS_PER_SOL, 0, 0],
        array::from_fn(|_| Pubkey::new_unique()),
    )
    .await;

    post_withdraw(&server, &deposit_output.deposit_id, 0).await;

    let withdraw_wallet_balance_before = env.rpc.get_balance(&withdraw_wallet).await.unwrap();

    let zk_deposit_data_before = server
        .state
        .zk_deposit_data_writer
        .get(&deposit_output.deposit_id)
        .await
        .unwrap();

    let (status, body) = server
        .post_json(
            "/withdraw",
            create_withdraw_body(&deposit_output.deposit_id, 0),
        )
        .await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("already withdrawn"),
        "{body}"
    );
    assert_eq!(
        env.rpc.get_balance(&withdraw_wallet).await.unwrap(),
        withdraw_wallet_balance_before,
        "no tx is sent"
    );
    assert_eq!(
        server
            .state
            .zk_deposit_data_writer
            .get(&deposit_output.deposit_id)
            .await
            .unwrap(),
        zk_deposit_data_before
    );
}

/// The deposit is not the last leaf, so its path has real siblings, not only zero values.
#[tokio::test]
#[ignore = "needs surfpool: run `just it`"]
async fn withdraw_after_other_deposits() {
    let env = SurfnetEnv::connect().await;
    env.ensure_pool_initialized().await;
    let server = env.start_server("withdraw_after_other_deposits").await;
    let recipients: [Pubkey; CHUNKS] = array::from_fn(|_| Pubkey::new_unique());

    let deposit_a_output = post_deposit(&server, [LAMPORTS_PER_SOL, 0, 0], recipients).await;
    for _ in 0..2 {
        post_deposit(
            &server,
            [2 * LAMPORTS_PER_SOL, 0, 0],
            array::from_fn(|_| Pubkey::new_unique()),
        )
        .await;
    }

    let withdraw_output = post_withdraw(&server, &deposit_a_output.deposit_id, 0).await;

    assert_eq!(withdraw_output.amount, LAMPORTS_PER_SOL);
    assert_eq!(
        env.rpc.get_balance(&recipients[0]).await.unwrap(),
        LAMPORTS_PER_SOL
    );
}

/// Invalid requests are refused before any tx is sent.
#[tokio::test]
#[ignore = "needs surfpool: run `just it`"]
async fn withdraw_rejects_invalid_requests() {
    let env = SurfnetEnv::connect().await;
    env.ensure_pool_initialized().await;
    let server = env.start_server("withdraw_rejects_invalid_requests").await;
    let withdraw_wallet = server.state.withdraw_wallets.default().pubkey();
    let deposit_wallet = server.state.deposit_wallets.default().pubkey();

    let deposit_output = post_deposit(
        &server,
        [LAMPORTS_PER_SOL, 0, LAMPORTS_PER_SOL],
        array::from_fn(|_| Pubkey::new_unique()),
    )
    .await;

    let withdraw_wallet_balance_before = env.rpc.get_balance(&withdraw_wallet).await.unwrap();

    let zero_chunk_body = create_withdraw_body(&deposit_output.deposit_id, 1);

    let unknown_deposit_body =
        create_withdraw_body(&hex::encode(Pubkey::new_unique().to_bytes()), 0);

    let step_out_of_range_body = create_withdraw_body(&deposit_output.deposit_id, CHUNKS);
    let mut deposit_wallet_fee_payer_body = create_withdraw_body(&deposit_output.deposit_id, 0);
    deposit_wallet_fee_payer_body["fee_payer_address"] = json!(deposit_wallet.to_string());

    for (body, expected_status, expected_error) in [
        (zero_chunk_body, StatusCode::BAD_REQUEST, "zero chunk"),
        (unknown_deposit_body, StatusCode::NOT_FOUND, "not found"),
        (
            step_out_of_range_body,
            StatusCode::BAD_REQUEST,
            "step must be below",
        ),
        (
            deposit_wallet_fee_payer_body,
            StatusCode::BAD_REQUEST,
            "not a configured withdraw wallet",
        ),
    ] {
        let (status, body) = server.post_json("/withdraw", body).await;
        assert_eq!(status, expected_status, "{body}");
        assert!(
            body["error"].as_str().unwrap().contains(expected_error),
            "{body}"
        );
    }

    assert_eq!(
        env.rpc.get_balance(&withdraw_wallet).await.unwrap(),
        withdraw_wallet_balance_before,
        "no tx is sent"
    );
}

/// Only a `Confirmed` deposit is withdrawn; the 409 points to reconcile and the status stays.
#[tokio::test]
#[ignore = "needs surfpool: run `just it`"]
async fn withdraw_wrong_status() {
    let env = SurfnetEnv::connect().await;
    env.ensure_pool_initialized().await;
    let server = env.start_server("withdraw_wrong_status").await;
    let withdraw_wallet = server.state.withdraw_wallets.default().pubkey();

    let deposit_output = post_deposit(
        &server,
        [LAMPORTS_PER_SOL, 0, 0],
        array::from_fn(|_| Pubkey::new_unique()),
    )
    .await;

    let withdraw_wallet_balance_before = env.rpc.get_balance(&withdraw_wallet).await.unwrap();

    for wrong_status in [ZkDepositStatus::Pending, ZkDepositStatus::Failed] {
        let zk_deposit_data = server
            .state
            .zk_deposit_data_writer
            .update(&deposit_output.deposit_id, |zk_deposit_data| {
                zk_deposit_data.status = wrong_status;
            })
            .await
            .unwrap();

        let (status, body) = server
            .post_json(
                "/withdraw",
                create_withdraw_body(&deposit_output.deposit_id, 0),
            )
            .await;

        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        let error = body["error"].as_str().unwrap();
        assert!(error.contains(&format!("{wrong_status:?}")), "{error}");
        assert!(
            error.contains(&format!("/deposit/{}/reconcile", deposit_output.deposit_id)),
            "{error}"
        );
        assert_eq!(
            server
                .state
                .zk_deposit_data_writer
                .get(&deposit_output.deposit_id)
                .await
                .unwrap(),
            zk_deposit_data
        );
    }

    assert_eq!(
        env.rpc.get_balance(&withdraw_wallet).await.unwrap(),
        withdraw_wallet_balance_before,
        "no tx is sent"
    );
}

/// Lamports sent to the nullifier PDA before the withdraw leave the step withdrawable.
#[tokio::test]
#[ignore = "needs surfpool: run `just it`"]
async fn withdraw_with_prefunded_nullifier_pda() {
    let env = SurfnetEnv::connect().await;
    env.ensure_pool_initialized().await;

    let server = env
        .start_server("withdraw_with_prefunded_nullifier_pda")
        .await;

    let recipients: [Pubkey; CHUNKS] = array::from_fn(|_| Pubkey::new_unique());

    let deposit_output = post_deposit(&server, [LAMPORTS_PER_SOL, 0, 0], recipients).await;
    let secret_s = deposit_output.zk_deposit_data.secret().unwrap();
    let nullifier_pda = derive_nullifier_pda(&fr_to_be(&create_nullifier(secret_s, 0)));
    env.fund_account_with_lamports(&nullifier_pda, 1_000_000)
        .await;

    post_withdraw(&server, &deposit_output.deposit_id, 0).await;

    assert_eq!(
        env.rpc.get_balance(&recipients[0]).await.unwrap(),
        LAMPORTS_PER_SOL
    );

    let nullifier_account = env
        .rpc
        .get_account_with_commitment(&nullifier_pda, RPC_COMMITMENT)
        .await
        .unwrap()
        .value
        .expect("the withdraw creates the nullifier account");

    assert_eq!(nullifier_account.owner, ZK_SHIELDED_POOL_PROGRAM_ID);
}

/// A withdraw tx the program rejects after both uploads still gets the proof account rent back.
#[tokio::test]
#[ignore = "needs surfpool: run `just it`"]
async fn rejected_withdraw_closes_the_proof_account() {
    let env = SurfnetEnv::connect().await;
    env.ensure_pool_initialized().await;

    let server = env
        .start_server("rejected_withdraw_closes_the_proof_account")
        .await;

    let withdraw_wallet = server.state.withdraw_wallets.default().pubkey();

    // The program refuses the vault as recipient (`RecipientIsVault`).
    let recipients = [
        derive_vault_pda(),
        Pubkey::new_unique(),
        Pubkey::new_unique(),
    ];

    let deposit_output = post_deposit(&server, [LAMPORTS_PER_SOL, 0, 0], recipients).await;
    let withdraw_wallet_balance_before = env.rpc.get_balance(&withdraw_wallet).await.unwrap();

    let (status, body) = server
        .post_json(
            "/withdraw",
            create_withdraw_body(&deposit_output.deposit_id, 0),
        )
        .await;

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(body["error"], "transaction rejected", "{body}");

    // Upload part 0, upload part 1 and close_proof; the rejected withdraw tx fails preflight.
    assert_eq!(
        env.rpc.get_balance(&withdraw_wallet).await.unwrap(),
        withdraw_wallet_balance_before - 3 * LAMPORTS_PER_SIGNATURE
    );

    let stored_zk_deposit_data = server
        .state
        .zk_deposit_data_writer
        .get(&deposit_output.deposit_id)
        .await
        .unwrap();

    assert!(stored_zk_deposit_data.withdrawals[0].is_none());
}
