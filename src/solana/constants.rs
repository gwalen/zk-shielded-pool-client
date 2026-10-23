use solana_commitment_config::{CommitmentConfig, CommitmentLevel};

pub const RPC_COMMITMENT: CommitmentConfig = CommitmentConfig { commitment: CommitmentLevel::Confirmed };
