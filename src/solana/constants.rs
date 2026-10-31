use std::time::Duration;

use solana_commitment_config::{CommitmentConfig, CommitmentLevel};

pub const RPC_COMMITMENT: CommitmentConfig = CommitmentConfig {
    commitment: CommitmentLevel::Confirmed,
};

/// A node can answer `get_transaction` with a brief error right after confirming the tx.
pub const GET_TRANSACTION_ATTEMPTS: usize = 5;
pub const GET_TRANSACTION_RETRY_DELAY: Duration = Duration::from_millis(500);
