use std::str::FromStr;

use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;

use crate::{
    app_error::{AppError, RequestResult},
    service::deposit::DepositRequest,
    solana::constants::MIN_CHUNK_LAMPORTS,
    state::UserWallets,
};

#[derive(Debug, Serialize, Deserialize)]
pub struct DepositInput {
    /// Lamports per destination: 0 or at least `MIN_CHUNK_LAMPORTS`.
    pub chunks: [u64; 3],
    /// base58 recipients, one per chunk.
    pub withdraw_addresses: [String; 3],
    /// base58 address of a configured deposit wallet: it signs and pays the deposit.
    // #[serde(with = "hex::serde")] - if this would be [u8;32] and I would need to parse it from hex string
    pub deposit_wallet_address: String,
}

impl DepositInput {
    /// Checks the wallet, the addresses, the chunks and the sum (400 on failure).
    pub fn into_request(self, deposit_wallets: &UserWallets) -> RequestResult<DepositRequest> {
        let depositor = deposit_wallets.parse_and_get(&self.deposit_wallet_address)?;
        let destinations = parse_destinations(&self.withdraw_addresses)?;
        check_chunks_reach_minimum(&self.chunks)?;

        let total_amount = self
            .chunks
            .iter()
            .try_fold(0u64, |sum, chunk| sum.checked_add(*chunk))
            .ok_or_else(|| AppError::BadRequest("the sum of the chunks overflows u64".into()))?;

        Ok(DepositRequest {
            depositor,
            chunks: self.chunks,
            destinations,
            total_amount,
        })
    }
}

fn parse_destinations(addresses: &[String; 3]) -> RequestResult<[Pubkey; 3]> {
    let mut destinations = [Pubkey::default(); 3];
    for (position, (destination, address)) in destinations.iter_mut().zip(addresses).enumerate() {
        *destination = Pubkey::from_str(address.trim()).map_err(|_| {
            AppError::BadRequest(format!(
                "withdraw address #{} is not a valid public key",
                position + 1
            ))
        })?;
    }
    Ok(destinations)
}

/// A recipient paid a chunk of at least the minimum always stays rent-exempt.
fn check_chunks_reach_minimum(chunks: &[u64; 3]) -> RequestResult<()> {
    for (position, chunk_lamports) in chunks.iter().enumerate() {
        if *chunk_lamports != 0 && *chunk_lamports < MIN_CHUNK_LAMPORTS {
            return Err(AppError::BadRequest(format!(
                "chunk #{} is {chunk_lamports} lamports, it must be 0 or at least {MIN_CHUNK_LAMPORTS}",
                position + 1
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{array, sync::Arc};

    use solana_keypair::Keypair;
    use solana_signer::Signer;

    use super::*;
    use crate::state::WalletKind;

    fn create_deposit_input(
        chunks: [u64; 3],
        withdraw_addresses: [String; 3],
        wallet: &UserWallets,
    ) -> DepositInput {
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

        let deposit_request = create_deposit_input(
            [2_000_000, 0, 5_000_000],
            create_withdraw_addresses(),
            &deposit_wallets,
        )
        .into_request(&deposit_wallets)
        .unwrap();

        assert_eq!(deposit_request.total_amount, 7_000_000);
        assert_eq!(
            deposit_request.depositor.pubkey(),
            deposit_wallets.default().pubkey()
        );
    }

    #[test]
    fn bad_withdraw_address_is_rejected() {
        let deposit_wallets = create_deposit_wallets();
        let mut withdraw_addresses = create_withdraw_addresses();
        withdraw_addresses[1] = "not-a-key".into();

        let bad_withdraw_address_error = create_deposit_input(
            [MIN_CHUNK_LAMPORTS; 3],
            withdraw_addresses,
            &deposit_wallets,
        )
        .into_request(&deposit_wallets)
        .err()
        .unwrap();

        assert!(
            matches!(bad_withdraw_address_error, AppError::BadRequest(ref error_message) if error_message.contains("#2")),
            "{bad_withdraw_address_error}"
        );
    }

    #[test]
    fn overflowing_sum_is_rejected() {
        let deposit_wallets = create_deposit_wallets();

        let overflow_error = create_deposit_input(
            [u64::MAX, MIN_CHUNK_LAMPORTS, 0],
            create_withdraw_addresses(),
            &deposit_wallets,
        )
        .into_request(&deposit_wallets)
        .err()
        .unwrap();

        assert!(
            matches!(overflow_error, AppError::BadRequest(ref error_message) if error_message.contains("overflows")),
            "{overflow_error}"
        );
    }

    #[test]
    fn chunk_must_be_zero_or_reach_the_minimum() {
        let deposit_wallets = create_deposit_wallets();

        let below_minimum_error = create_deposit_input(
            [MIN_CHUNK_LAMPORTS, MIN_CHUNK_LAMPORTS - 1, 0],
            create_withdraw_addresses(),
            &deposit_wallets,
        )
        .into_request(&deposit_wallets)
        .err()
        .unwrap();

        assert!(
            matches!(below_minimum_error, AppError::BadRequest(ref error_message) if error_message.contains("#2") && error_message.contains(&MIN_CHUNK_LAMPORTS.to_string())),
            "{below_minimum_error}"
        );

        let at_minimum_deposit_request = create_deposit_input(
            [MIN_CHUNK_LAMPORTS, MIN_CHUNK_LAMPORTS, 0],
            create_withdraw_addresses(),
            &deposit_wallets,
        )
        .into_request(&deposit_wallets)
        .unwrap();

        assert_eq!(
            at_minimum_deposit_request.total_amount,
            2 * MIN_CHUNK_LAMPORTS
        );
    }
}
