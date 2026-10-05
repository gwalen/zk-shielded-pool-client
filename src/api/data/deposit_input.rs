use serde::{Serialize, Deserialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct DepositInput {
    pub chunks: [u64; 3],
    pub withdraw_addresses: [String; 3],
    // #[serde(with = "hex::serde")] - if this would be [u8;32] and I would need to parse it from hex string
    pub deposit_wallet_address: String,
}