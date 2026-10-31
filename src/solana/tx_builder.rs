//! V0 transactions only: compile, sign, send, confirm, read logs.
//!
//! Same shape as `signed_v0_tx` / `send_tx_expect_ok` in r3-session-keys, but async and against
//! RPC. `compile_v0` has no RPC call, so LiteSVM tests use it too.

use solana_hash::Hash;
use solana_instruction::{Instruction, error::InstructionError};
use solana_keypair::Keypair;
use solana_message::{VersionedMessage, v0};
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_rpc_client_api::{client_error::Error as ClientError, config::RpcTransactionConfig};
use solana_signature::Signature;
use solana_signer::Signer;
use solana_transaction::versioned::VersionedTransaction;
use solana_transaction_error::TransactionError;
use solana_transaction_status_client_types::{
    EncodedConfirmedTransactionWithStatusMeta, UiTransactionEncoding,
};
use tokio::time::sleep;
use tracing::warn;

use super::{
    constants::{GET_TRANSACTION_ATTEMPTS, GET_TRANSACTION_RETRY_DELAY},
    program_helpers::get_error_name,
};
use crate::app_error::{AppError, RequestResult};

/// Build and sign a V0 transaction. The payer is the fee payer and the only signer: every
/// instruction this client sends needs exactly one signature (the deposit or withdraw wallet).
pub fn compile_v0(
    payer: &Keypair,
    ixs: &[Instruction],
    blockhash: Hash,
) -> RequestResult<VersionedTransaction> {
    let msg = v0::Message::try_compile(&payer.pubkey(), ixs, &[] /* no LUTs */, blockhash)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("compile v0 message: {e}")))?;
    VersionedTransaction::try_new(VersionedMessage::V0(msg), &[payer])
        .map_err(|e| AppError::Internal(anyhow::anyhow!("sign v0 transaction: {e}")))
}

/// Send a V0 transaction and wait for the RPC client's commitment (`RPC_COMMITMENT`).
pub async fn send_v0(
    rpc: &RpcClient,
    payer: &Keypair,
    ixs: &[Instruction],
) -> RequestResult<Signature> {
    let blockhash = rpc.get_latest_blockhash().await.map_err(create_rpc_error)?;
    let tx = compile_v0(payer, ixs, blockhash)?;
    rpc.send_and_confirm_transaction(&tx)
        .await
        .map_err(create_rpc_error)
}

/// Log messages of a confirmed transaction, retrying `get_transaction` a bounded number of times.
pub async fn transaction_logs(
    rpc: &RpcClient,
    signature: &Signature,
) -> RequestResult<Vec<String>> {
    let config = RpcTransactionConfig {
        encoding: Some(UiTransactionEncoding::Base64),
        commitment: Some(rpc.commitment()),
        // Required: without it the RPC rejects V0 transactions.
        max_supported_transaction_version: Some(0),
    };
    let tx = fetch_transaction_with_retry(rpc, signature, config).await?;

    let logs = tx
        .transaction
        .meta
        .and_then(|meta| Option::<Vec<String>>::from(meta.log_messages))
        .ok_or_else(|| AppError::RpcError(format!("transaction {signature} has no logs")))?;

    Ok(logs)
}

async fn fetch_transaction_with_retry(
    rpc: &RpcClient,
    signature: &Signature,
    config: RpcTransactionConfig,
) -> RequestResult<EncodedConfirmedTransactionWithStatusMeta> {
    let mut attempt = 1;
    loop {
        match rpc.get_transaction_with_config(signature, config).await {
            Ok(tx) => return Ok(tx),
            Err(error) if attempt == GET_TRANSACTION_ATTEMPTS => {
                return Err(create_rpc_error(error));
            }
            Err(error) => {
                warn!(%signature, attempt, %error, "get_transaction failed, retrying");
                attempt += 1;
                sleep(GET_TRANSACTION_RETRY_DELAY).await;
            }
        }
    }
}

/// Preflight and on-chain failures -> `AppError::TxRejected`, naming the program error when there
/// is one. Every other failure -> `AppError::RpcError`: the tx may still have landed.
pub fn create_rpc_error(error: ClientError) -> AppError {
    match error.get_transaction_error() {
        Some(tx_error) => AppError::TxRejected(parse_transaction_error(&tx_error)),
        None => AppError::RpcError(error.to_string()),
    }
}

pub fn parse_transaction_error(error: &TransactionError) -> String {
    match error {
        TransactionError::InstructionError(index, InstructionError::Custom(code)) => {
            match get_error_name(*code) {
                Some(name) => format!(
                    "instruction {index} failed: zk-shielded-pool program error {code} ({name})"
                ),
                None => format!("instruction {index} failed: custom error {code}"),
            }
        }
        other => format!("transaction failed: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use solana_rpc_client_api::{client_error::ErrorKind, request::RpcError};

    use super::*;

    /// An on-chain failure is a definite rejection; "unable to confirm" leaves the outcome unknown.
    #[test]
    fn create_rpc_error_separates_rejections_from_unknown_outcomes() {
        let on_chain_failure = ErrorKind::TransactionError(TransactionError::AccountNotFound);
        assert!(matches!(
            create_rpc_error(on_chain_failure.into()),
            AppError::TxRejected(_)
        ));

        let unable_to_confirm =
            ErrorKind::RpcError(RpcError::ForUser("unable to confirm transaction".into()));
        assert!(matches!(
            create_rpc_error(unable_to_confirm.into()),
            AppError::RpcError(_)
        ));
    }

    #[test]
    fn check_transaction_error_parsing() {
        // Our program error codes (defined as AppErrors in the program) are in the 6000s and we give them additional context from the IDL
        let error = TransactionError::InstructionError(2, InstructionError::Custom(6009));
        assert_eq!(
            parse_transaction_error(&error),
            "instruction 2 failed: zk-shielded-pool program error 6009 (UnknownRoot)"
        );
        // Custom errors from Solana runtime
        let error = TransactionError::InstructionError(0, InstructionError::Custom(42));
        assert_eq!(
            parse_transaction_error(&error),
            "instruction 0 failed: custom error 42"
        );
    }
}
