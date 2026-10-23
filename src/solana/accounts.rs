use solana_pubkey::Pubkey;

use super::accounts_helpers::decode_account;
use super::program_helpers::{ProgramConfig, RootRegistry};
use crate::app_error::RequestResult;

/// Root history length on chain (`ROOT_RING_BUFFER_LENGTH`).
pub const ROOT_HISTORY_LEN: usize = 100;

/// Marker for an unused root-history slot: `Fr(1)` in LE. Never a known root.
pub const EMPTY_TREE_VALUE: [u8; 32] = {
    let mut bytes = [0u8; 32];
    bytes[0] = 1;
    bytes
};

/// Small views over the generated account types, so the rest of the crate does not depend on
/// generated field types (`PodU64`, `PodBool`, ...) from IDL
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramConfigView {
    pub owner: Pubkey,
    pub paused: bool,
    pub min_deposit_lamports: u64,
}

impl ProgramConfigView {
    pub fn decode(data: &[u8]) -> RequestResult<Self> {
        let config: ProgramConfig = decode_account(data, "ProgramConfig")?;
        Ok(Self {
            owner: config.owner,
            paused: config.pause.get(),
            min_deposit_lamports: config.min_deposit_lamports.get(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootRegistryView {
    /// Current tree root, LE.
    pub root_le: [u8; 32],
    pub next_leaf_idx: u32,
    /// The 100-slot ring buffer as stored, LE. Unused slots hold `EMPTY_TREE_VALUE`.
    pub roots_history: Vec<[u8; 32]>,
}

impl RootRegistryView {
    pub fn decode(data: &[u8]) -> RequestResult<Self> {
        let registry: RootRegistry = decode_account(data, "RootRegistry")?;
        let roots_history = registry
            .roots_history
            .chunks_exact(32)
            .map(|root| <[u8; 32]>::try_from(root).expect("chunks_exact(32)"))
            .collect();

        Ok(Self {
            root_le: registry.imt.root,
            next_leaf_idx: registry.imt.next_leaf_idx.get(),
            roots_history,
        })
    }

    /// Same rule as the program's `RootRegistry::is_known_root`.
    /// check if root belongs to the history
    pub fn is_known_root(&self, root_le: &[u8; 32]) -> bool {
        *root_le != EMPTY_TREE_VALUE && self.roots_history.contains(root_le)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anchor_lang::Discriminator;

    fn registry_bytes(root: [u8; 32], next_leaf_idx: u32, history: &[[u8; 32]]) -> Vec<u8> {
        let mut data = RootRegistry::DISCRIMINATOR.to_vec();
        data.extend_from_slice(&root);
        data.extend_from_slice(&[0u8; 640 * 2]); // frontiers + zero values
        data.extend_from_slice(&next_leaf_idx.to_le_bytes());
        for slot in 0..ROOT_HISTORY_LEN {
            data.extend_from_slice(history.get(slot).unwrap_or(&EMPTY_TREE_VALUE));
        }
        data.extend_from_slice(&0u32.to_le_bytes()); // last_root_idx
        data.push(255); // bump
        data
    }

    #[test]
    fn decode_rejects_wrong_discriminator_or_short_data() {
        let mut data = registry_bytes([7; 32], 0, &[]);
        assert!(ProgramConfigView::decode(&data).is_err());
        data[0] ^= 1;
        assert!(RootRegistryView::decode(&data).is_err());
        let short = &registry_bytes([7; 32], 0, &[])[..100];
        assert!(RootRegistryView::decode(short).is_err());
    }
}
