use serde::{Deserialize, Serialize};

use crate::service::withdraw::WithdrawReceipt;

#[derive(Serialize, Deserialize)]
pub struct WithdrawOutput {
    /// base58
    pub withdraw_tx: String,
    /// base58, upload part 0 then part 1.
    pub upload_txs: [String; 2],
    /// base58. `None` if closing the proof account failed.
    pub close_proof_tx: Option<String>,
    /// base58
    pub recipient: String,
    /// Lamports paid to the recipient.
    pub amount: u64,
    /// hex BE, the nullifier account seed.
    pub nullifier: String,
}

impl From<WithdrawReceipt> for WithdrawOutput {
    fn from(receipt: WithdrawReceipt) -> Self {
        Self {
            withdraw_tx: receipt.withdraw_tx.to_string(),
            upload_txs: receipt.upload_txs.map(|upload_tx| upload_tx.to_string()),
            close_proof_tx: receipt
                .close_proof_tx
                .map(|close_proof_tx| close_proof_tx.to_string()),
            recipient: receipt.recipient.to_string(),
            amount: receipt.amount,
            nullifier: hex::encode(receipt.nullifier_be),
        }
    }
}
