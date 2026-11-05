//! Program events decoded from transaction logs.

use solana_rpc_client::{
    nonblocking::rpc_client::RpcClient, rpc_client::GetConfirmedSignaturesForAddress2Config,
};
use solana_rpc_client_api::request::MAX_GET_CONFIRMED_SIGNATURES_FOR_ADDRESS2_LIMIT;
use solana_signature::Signature;

use super::{
    program_helpers::{ZK_SHIELDED_POOL_PROGRAM_ID, events},
    tx_builder::{create_rpc_error, transaction_logs},
};
use crate::app_error::{AppError, RequestResult};

pub use events::DepositDone;

/// All `DepositDone` events our program emitted in `logs`.
///
/// `parse_logs` tracks the invoke stack, so `Program data:` lines written by other programs
/// (even with a matching discriminator) are ignored.
pub fn parse_deposit_done_events(logs: &[String]) -> RequestResult<Vec<DepositDone>> {
    anchor_client::parse_logs::<DepositDone>(logs, &ZK_SHIELDED_POOL_PROGRAM_ID.to_string())
        .map_err(|e| AppError::RpcError(format!("cannot parse DepositDone logs: {e}")))
}

/// Every `DepositDone` of the program with its slot, oldest first, read from the full tx history.
pub async fn fetch_deposit_done_events(rpc: &RpcClient) -> RequestResult<Vec<(u64, DepositDone)>> {
    let mut successful_tx_statuses = Vec::new();
    let mut before = None;

    loop {
        let signatures_request_config = GetConfirmedSignaturesForAddress2Config {
            before,
            until: None,
            limit: Some(MAX_GET_CONFIRMED_SIGNATURES_FOR_ADDRESS2_LIMIT),
            commitment: Some(rpc.commitment()),
        };

        // Newest first.
        let tx_statuses = rpc
            .get_signatures_for_address_with_config(
                &ZK_SHIELDED_POOL_PROGRAM_ID,
                signatures_request_config,
            )
            .await
            .map_err(create_rpc_error)?;

        let is_last_page = tx_statuses.len() < MAX_GET_CONFIRMED_SIGNATURES_FOR_ADDRESS2_LIMIT;
        before = tx_statuses
            .last()
            .map(|tx_status| parse_signature(&tx_status.signature))
            .transpose()?;
        successful_tx_statuses.extend(
            tx_statuses
                .into_iter()
                .filter(|tx_status| tx_status.err.is_none()),
        );

        if is_last_page {
            break;
        }
    }

    let mut deposit_done_events = Vec::new();
    for tx_status in successful_tx_statuses.into_iter().rev() {
        let logs = transaction_logs(rpc, &parse_signature(&tx_status.signature)?).await?;
        for deposit_done in parse_deposit_done_events(&logs)? {
            deposit_done_events.push((tx_status.slot, deposit_done));
        }
    }

    Ok(deposit_done_events)
}

fn parse_signature(signature: &str) -> RequestResult<Signature> {
    signature.parse().map_err(|_| {
        AppError::RpcError(format!(
            "getSignaturesForAddress returned an invalid signature: {signature}"
        ))
    })
}
