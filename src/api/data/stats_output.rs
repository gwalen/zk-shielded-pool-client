use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;

use crate::zk::zk_deposit_data::{ZkDepositData, ZkDepositStatus};

/// Counts of stored zk deposits and the wallets this server uses.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatsOutput {
    /// Confirmed deposits that still have a funded chunk to withdraw.
    pub open_zk_deposit_count: u64,
    /// Deposits whose status is pending.
    pub pending_zk_deposit_count: u64,
    /// Confirmed deposits with a withdrawal stored for every funded chunk.
    pub withdrawn_zk_deposit_count: u64,
    /// Funded chunks still to withdraw on confirmed deposits.
    pub open_chunk_count: u64,
    /// Funded chunks on pending deposits.
    pub pending_chunk_count: u64,
    /// Chunks with a withdrawal record, across every stored deposit.
    pub withdrawn_chunk_count: u64,
    /// base58 public keys of the configured deposit wallets, in config order.
    pub deposit_wallets: Vec<String>,
    /// base58 public keys of the configured withdraw wallets, in config order.
    pub withdraw_wallets: Vec<String>,
}

impl StatsOutput {
    pub fn from_zk_deposit_data_and_wallets(
        zk_deposit_data_list: &[ZkDepositData],
        deposit_wallet_pubkeys: &[Pubkey],
        withdraw_wallet_pubkeys: &[Pubkey],
    ) -> Self {
        let mut open_zk_deposit_count = 0u64;
        let mut pending_zk_deposit_count = 0u64;
        let mut withdrawn_zk_deposit_count = 0u64;
        let mut open_chunk_count = 0u64;
        let mut pending_chunk_count = 0u64;
        let mut withdrawn_chunk_count = 0u64;

        for zk_deposit_data in zk_deposit_data_list {
            open_chunk_count += zk_deposit_data.open_chunk_count();
            pending_chunk_count += zk_deposit_data.pending_chunk_count();
            withdrawn_chunk_count += zk_deposit_data.withdrawn_chunk_count();

            if zk_deposit_data.is_open() {
                open_zk_deposit_count += 1;
            }
            if zk_deposit_data.status == ZkDepositStatus::Pending {
                pending_zk_deposit_count += 1;
            }
            if zk_deposit_data.is_fully_withdrawn() {
                withdrawn_zk_deposit_count += 1;
            }
        }

        let deposit_wallets = deposit_wallet_pubkeys
            .iter()
            .map(ToString::to_string)
            .collect();

        let withdraw_wallets = withdraw_wallet_pubkeys
            .iter()
            .map(ToString::to_string)
            .collect();

        Self {
            open_zk_deposit_count,
            pending_zk_deposit_count,
            withdrawn_zk_deposit_count,
            open_chunk_count,
            pending_chunk_count,
            withdrawn_chunk_count,
            deposit_wallets,
            withdraw_wallets,
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::zk::zk_deposit_data::{WithdrawalRecord, ZkDepositStatus};

    fn create_stats_deposit(
        deposit_id_byte: u8,
        chunks: [u64; 3],
        withdrawn_steps: &[usize],
    ) -> ZkDepositData {
        let mut withdrawals: [Option<WithdrawalRecord>; 3] = [None, None, None];
        for step in withdrawn_steps {
            withdrawals[*step] = Some(WithdrawalRecord {
                tx: Some("sig".into()),
                nullifier_hex: hex::encode([0u8; 32]),
                at: Utc::now(),
            });
        }

        ZkDepositData {
            id: hex::encode([deposit_id_byte; 32]),
            secret_s: "00".repeat(32),
            deposit_wallet: "Wallet1111111111111111111111111111111111111".into(),
            total_amount: chunks.iter().sum(),
            chunks,
            destinations: ["a".into(), "b".into(), "c".into()],
            user_commitment_hash: hex::encode([1; 32]),
            deposit_commitment_hash: hex::encode([deposit_id_byte; 32]),
            status: ZkDepositStatus::Confirmed,
            deposit_tx: "sig".into(),
            error: None,
            created_at: Utc::now(),
            withdrawals,
        }
    }

    #[test]
    fn from_zk_deposit_data_and_wallets_sums_open_and_withdrawn() {
        let deposit_wallet_a = Pubkey::new_unique();
        let deposit_wallet_b = Pubkey::new_unique();
        let withdraw_wallet = Pubkey::new_unique();

        let empty = StatsOutput::from_zk_deposit_data_and_wallets(
            &[],
            &[deposit_wallet_a],
            &[withdraw_wallet],
        );

        assert_eq!(empty.open_zk_deposit_count, 0);
        assert_eq!(empty.pending_zk_deposit_count, 0);
        assert_eq!(empty.withdrawn_zk_deposit_count, 0);
        assert_eq!(empty.open_chunk_count, 0);
        assert_eq!(empty.pending_chunk_count, 0);
        assert_eq!(empty.withdrawn_chunk_count, 0);
        assert_eq!(empty.deposit_wallets, vec![deposit_wallet_a.to_string()]);
        assert_eq!(empty.withdraw_wallets, vec![withdraw_wallet.to_string()]);

        let mut pending_zk_deposit_data = create_stats_deposit(5, [9, 0, 0], &[]);

        pending_zk_deposit_data.status = ZkDepositStatus::Pending;

        let mut failed_zk_deposit_data = create_stats_deposit(6, [7, 0, 8], &[]);
        // A rejected deposit keeps its funded chunks and empty withdrawal slots.
        failed_zk_deposit_data.status = ZkDepositStatus::Failed;

        let zk_deposit_data_list = vec![
            create_stats_deposit(1, [2, 0, 4], &[0]),
            create_stats_deposit(2, [1, 1, 1], &[0, 1, 2]),
            create_stats_deposit(3, [5, 5, 5], &[]),
            create_stats_deposit(4, [0, 0, 0], &[]),
            pending_zk_deposit_data,
            failed_zk_deposit_data,
        ];

        let stats = StatsOutput::from_zk_deposit_data_and_wallets(
            &zk_deposit_data_list,
            &[deposit_wallet_a, deposit_wallet_b],
            &[withdraw_wallet],
        );

        assert_eq!(stats.open_zk_deposit_count, 2);
        assert_eq!(stats.pending_zk_deposit_count, 1);
        assert_eq!(stats.withdrawn_zk_deposit_count, 1);
        assert_eq!(stats.open_chunk_count, 4);
        assert_eq!(stats.pending_chunk_count, 1);
        assert_eq!(stats.withdrawn_chunk_count, 4);
        assert_eq!(
            stats.deposit_wallets,
            vec![deposit_wallet_a.to_string(), deposit_wallet_b.to_string()]
        );
        assert_eq!(stats.withdraw_wallets, vec![withdraw_wallet.to_string()]);
    }
}
