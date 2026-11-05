use std::time::Duration;

use solana_commitment_config::{CommitmentConfig, CommitmentLevel};

pub const RPC_COMMITMENT: CommitmentConfig = CommitmentConfig {
    commitment: CommitmentLevel::Confirmed,
};

/// A node can answer `get_transaction` with a brief error right after confirming the tx.
pub const GET_TRANSACTION_ATTEMPTS: usize = 5;
pub const GET_TRANSACTION_RETRY_DELAY: Duration = Duration::from_millis(500);

/// Smallest non-zero chunk: `Rent::minimum_balance(0)` with the default rent.
pub const MIN_CHUNK_LAMPORTS: u64 = 890_880;
