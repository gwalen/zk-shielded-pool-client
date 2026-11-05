use serde::{Deserialize, Serialize};

use crate::{
    app_error::{AppError, RequestResult},
    service::withdraw::WithdrawRequest,
    state::UserWallets,
    zk::field::CHUNKS,
};

#[derive(Debug, Serialize, Deserialize)]
pub struct WithdrawInput {
    /// hex(deposit_commitment LE), from the deposit response.
    pub deposit_id: String,
    /// Which chunk to withdraw: 0, 1 or 2.
    pub step: u8,
    /// base58 address of a configured withdraw wallet that signs and pays the fees. The first
    /// configured withdraw wallet when missing.
    pub fee_payer_address: Option<String>,
}

impl WithdrawInput {
    /// Checks the step and the fee payer (400 on failure).
    pub fn into_request(self, withdraw_wallets: &UserWallets) -> RequestResult<WithdrawRequest> {
        if usize::from(self.step) >= CHUNKS {
            return Err(AppError::BadRequest(format!(
                "step must be below {CHUNKS}, got {}",
                self.step
            )));
        }

        let fee_payer = match &self.fee_payer_address {
            Some(fee_payer_address) => withdraw_wallets.parse_and_get(fee_payer_address)?,
            None => withdraw_wallets.default(),
        };

        Ok(WithdrawRequest {
            deposit_id: self.deposit_id,
            step: self.step,
            fee_payer,
        })
    }
}
