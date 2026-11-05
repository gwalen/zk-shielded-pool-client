//! Fetch and decode the program accounts over RPC.

use anchor_lang::AccountDeserialize;
use solana_pubkey::Pubkey;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;

use super::accounts::{ProgramConfigView, RootRegistryView};
use super::program_helpers::{
    Nullifier, ZK_SHIELDED_POOL_PROGRAM_ID, derive_program_config_pda, derive_root_registry_pda,
};
use super::tx_builder::create_rpc_error;
use crate::app_error::{AppError, RequestResult};

/// `None` if the pool is not initialized yet.
pub async fn fetch_program_config(rpc: &RpcClient) -> RequestResult<Option<ProgramConfigView>> {
    fetch_account_data(rpc, &derive_program_config_pda())
        .await?
        .as_deref()
        .map(ProgramConfigView::decode)
        .transpose()
}

/// `None` if the pool is not initialized yet.
pub async fn fetch_root_registry(rpc: &RpcClient) -> RequestResult<Option<RootRegistryView>> {
    fetch_account_data(rpc, &derive_root_registry_pda())
        .await?
        .as_deref()
        .map(RootRegistryView::decode)
        .transpose()
}

/// The accounts a withdrawal checks before proving.
pub struct WithdrawalAccounts {
    pub nullifier_spent: bool,
    /// `None` if the pool is not initialized yet.
    pub program_config_account_view: Option<ProgramConfigView>,
}

/// The nullifier and program config accounts in one RPC call.
pub async fn fetch_withdrawal_accounts(
    rpc: &RpcClient,
    nullifier_pda: &Pubkey,
) -> RequestResult<WithdrawalAccounts> {
    let withdrawal_accounts = rpc
        .get_multiple_accounts_with_commitment(
            &[*nullifier_pda, derive_program_config_pda()],
            rpc.commitment(),
        )
        .await
        .map_err(create_rpc_error)?
        .value;

    let [nullifier_account, program_config_account] = &withdrawal_accounts[..] else {
        return Err(AppError::RpcError(format!(
            "get_multiple_accounts returned {} accounts for 2 addresses",
            withdrawal_accounts.len()
        )));
    };

    let program_config_account_view = program_config_account
        .as_ref()
        .map(|account| ProgramConfigView::decode(&account.data))
        .transpose()?;

    // Anyone can send lamports to the PDA, to block this kind of DDOS attack
    // we check if the PDA is owned by the our program and if it correctly deserializes to a `Nullifier`
    let nullifier_spent = match nullifier_account {
        Some(account) if account.owner == ZK_SHIELDED_POOL_PROGRAM_ID => {
            decode_account::<Nullifier>(&account.data, "Nullifier")?;
            true
        }
        _ => false,
    };

    Ok(WithdrawalAccounts {
        nullifier_spent,
        program_config_account_view,
    })
}

async fn fetch_account_data(rpc: &RpcClient, address: &Pubkey) -> RequestResult<Option<Vec<u8>>> {
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
