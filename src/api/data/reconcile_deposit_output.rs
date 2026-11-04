use serde::{Deserialize, Serialize};

use crate::zk::zk_deposit_data::{ZkDepositData, ZkDepositStatus};

#[derive(Serialize, Deserialize)]
pub struct ReconcileDepositOutput {
    /// hex(deposit_commitment LE)
    pub deposit_id: String,
    pub status: ZkDepositStatus,
    /// base58
    pub tx_signature: String,
    pub error: Option<String>,
}

impl From<ZkDepositData> for ReconcileDepositOutput {
    fn from(zk_deposit_data: ZkDepositData) -> Self {
        Self {
            deposit_id: zk_deposit_data.id,
            status: zk_deposit_data.status,
            tx_signature: zk_deposit_data.deposit_tx,
            error: zk_deposit_data.error,
        }
    }
}
