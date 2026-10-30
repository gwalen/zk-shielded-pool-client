use serde::{Deserialize, Serialize};

use crate::{service::deposit::DepositReceipt, zk::zk_deposit_data::ZkDepositData};

/// Holds the secret `s` (in `zk_deposit_data`): never log it.
#[derive(Serialize, Deserialize)]
pub struct DepositOutput {
    /// hex(deposit_commitment LE), the id for a later withdraw.
    pub deposit_id: String,
    /// base58
    pub tx_signature: String,
    pub total_amount: u64,
    /// Tree root after this deposit, hex LE.
    pub new_root: String,
    /// The full zk deposit data, also stored on the server.
    pub zk_deposit_data: ZkDepositData,
}

impl From<DepositReceipt> for DepositOutput {
    fn from(receipt: DepositReceipt) -> Self {
        Self {
            deposit_id: receipt.zk_deposit_data.id.clone(),
            tx_signature: receipt.tx_signature.to_string(),
            total_amount: receipt.zk_deposit_data.total_amount,
            new_root: hex::encode(receipt.new_root_le),
            zk_deposit_data: receipt.zk_deposit_data,
        }
    }
}
