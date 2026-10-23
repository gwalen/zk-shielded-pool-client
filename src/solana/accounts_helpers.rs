//! Fetch and decode the program accounts over RPC.

use anchor_lang::AccountDeserialize;
use solana_pubkey::Pubkey;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;

use super::accounts::{ProgramConfigView, RootRegistryView};
use super::program_helpers::{pda_program_config, pda_root_registry};
use crate::app_error::{AppError, RequestResult};

/// `None` if the pool is not initialized yet.
pub async fn fetch_program_config(rpc: &RpcClient) -> RequestResult<Option<ProgramConfigView>> {
    fetch_account_data(rpc, &pda_program_config())
        .await?
        .as_deref()
        .map(ProgramConfigView::decode)
        .transpose()
}

/// `None` if the pool is not initialized yet.
pub async fn fetch_root_registry(rpc: &RpcClient) -> RequestResult<Option<RootRegistryView>> {
    fetch_account_data(rpc, &pda_root_registry())
        .await?
        .as_deref()
        .map(RootRegistryView::decode)
        .transpose()
}

/// Whether an account exists (e.g. a nullifier PDA, which means that step was withdrawn).
pub async fn account_exists(rpc: &RpcClient, address: &Pubkey) -> RequestResult<bool> {
    Ok(fetch_account_data(rpc, address).await?.is_some())
}

async fn fetch_account_data(
    rpc: &RpcClient,
    address: &Pubkey,
) -> RequestResult<Option<Vec<u8>>> {
    let response = rpc
        .get_account_with_commitment(address, rpc.commitment())
        .await
        .map_err(|e| AppError::RpcError(format!("get_account {address}: {e}")))?;

    Ok(response.value.map(|account| account.data))
}

/// Generated `try_deserialize`: checks the discriminator and the length.
pub(super) fn decode_account<T: AccountDeserialize>(data: &[u8], name: &str) -> RequestResult<T> {
    let mut buf = data;

    T::try_deserialize(&mut buf)
        .map_err(|e| AppError::RpcError(format!("cannot decode {name} account: {e:?}")))
}
