use std::str::FromStr;

use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;

use crate::{
    app_error::{AppError, RequestResult},
    service::deposit::DepositRequest,
    state::UserWallets,
};

#[derive(Debug, Serialize, Deserialize)]
pub struct DepositInput {
    /// Lamports per destination. A chunk may be 0.
    pub chunks: [u64; 3],
    /// base58 recipients, one per chunk.
    pub withdraw_addresses: [String; 3],
    /// base58 address of a configured deposit wallet: it signs and pays the deposit.
    // #[serde(with = "hex::serde")] - if this would be [u8;32] and I would need to parse it from hex string
    pub deposit_wallet_address: String,
}

impl DepositInput {
    /// Checks the wallet, the addresses and the sum (400 on failure).
    pub fn into_request(self, deposit_wallets: &UserWallets) -> RequestResult<DepositRequest> {
        let depositor = deposit_wallets.parse_and_get(&self.deposit_wallet_address)?;
        let destinations = parse_destinations(&self.withdraw_addresses)?;
        let total_amount = self
            .chunks
            .iter()
            .try_fold(0u64, |sum, chunk| sum.checked_add(*chunk))
            .ok_or_else(|| AppError::BadRequest("the sum of the chunks overflows u64".into()))?;

        Ok(DepositRequest { depositor, chunks: self.chunks, destinations, total_amount })
    }
}

fn parse_destinations(addresses: &[String; 3]) -> RequestResult<[Pubkey; 3]> {
    let mut destinations = [Pubkey::default(); 3];
    for (position, (destination, address)) in destinations.iter_mut().zip(addresses).enumerate() {
        *destination = Pubkey::from_str(address.trim()).map_err(|_| {
            AppError::BadRequest(format!("withdraw address #{} is not a valid public key", position + 1))
        })?;
    }
    Ok(destinations)
}

#[cfg(test)]
mod tests {
    use std::{array, sync::Arc};

    use solana_keypair::Keypair;
    use solana_signer::Signer;

    use super::*;
    use crate::state::WalletKind;

    fn create_deposit_input(chunks: [u64; 3], withdraw_addresses: [String; 3], wallet: &UserWallets) -> DepositInput {
        DepositInput {
            chunks,
            withdraw_addresses,
            deposit_wallet_address: wallet.default().pubkey().to_string(),
        }
    }

    fn create_withdraw_addresses() -> [String; 3] {
        array::from_fn(|_| Pubkey::new_unique().to_string())
    }

    fn create_deposit_wallets() -> UserWallets {
        UserWallets::new(WalletKind::Deposit, &[Arc::new(Keypair::new())])
    }

    #[test]
    fn valid_input_sums_the_chunks() {
        let deposit_wallets = create_deposit_wallets();
        let request = create_deposit_input([2, 0, 5], create_withdraw_addresses(), &deposit_wallets).into_request(&deposit_wallets).unwrap();
        assert_eq!(request.total_amount, 7);
        assert_eq!(request.depositor.pubkey(), deposit_wallets.default().pubkey());
    }

    #[test]
    fn bad_withdraw_address_is_rejected() {
        let deposit_wallets = create_deposit_wallets();
        let mut withdraw_addresses = create_withdraw_addresses();
        withdraw_addresses[1] = "not-a-key".into();
        let err = create_deposit_input([1, 1, 1], withdraw_addresses, &deposit_wallets).into_request(&deposit_wallets).err().unwrap();
        assert!(matches!(err, AppError::BadRequest(ref msg) if msg.contains("#2")), "{err}");
    }

    #[test]
    fn overflowing_sum_is_rejected() {
        let deposit_wallets = create_deposit_wallets();
        let err = create_deposit_input([u64::MAX, 1, 0], create_withdraw_addresses(), &deposit_wallets).into_request(&deposit_wallets).err().unwrap();
        assert!(matches!(err, AppError::BadRequest(ref msg) if msg.contains("overflows")), "{err}");
    }
}
