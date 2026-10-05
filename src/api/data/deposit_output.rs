use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct DepositOutput {
    pub tx_hash: String,
}