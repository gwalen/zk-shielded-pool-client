use anyhow::format_err;
use ff::Field;
use shielded_pool_circuit::imt::imt_utils::{generate_zero_values_for_levels, poseidon_hash};

use super::field::{Fr, fr_from_le};
use crate::{
    app_error::{AppError, RequestResult},
    solana::events::DepositDone,
};

/// A deposit commitment the program inserted, with the tree root right after that insert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InsertedLeaf {
    pub slot: u64,
    pub leaf: Fr,
    pub new_root: Fr,
}

impl InsertedLeaf {
    pub fn from_deposit_done(slot: u64, deposit_done: &DepositDone) -> RequestResult<Self> {
        let leaf = fr_from_le(deposit_done.deposit_commitment_hash.to_bytes());
        let new_root = fr_from_le(deposit_done.new_root.to_bytes());

        match (leaf, new_root) {
            (Some(leaf), Some(new_root)) => Ok(Self {
                slot,
                leaf,
                new_root,
            }),
            _ => Err(AppError::RpcError(format!(
                "DepositDone in slot {slot} holds a value above the field modulus"
            ))),
        }
    }
}

/// Path from a leaf to the root; side `1` is a sibling on the right, as in the circuit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MerklePath<const DEPTH: usize> {
    pub siblings_path: [Fr; DEPTH],
    pub siblings_side: [u8; DEPTH],
    pub root: Fr,
}

impl<const DEPTH: usize> MerklePath<DEPTH> {
    /// Path of the leaf at `leaf_idx`; hashes only the nodes above the inserted leaves.
    pub fn from_leaves(leaves: &[Fr], leaf_idx: usize) -> Self {
        let zero_values = generate_zero_values_for_levels(DEPTH);
        let mut siblings_path = [Fr::ZERO; DEPTH];
        let mut siblings_side = [0u8; DEPTH];
        let mut level_nodes = leaves.to_vec();
        let mut node_idx = leaf_idx;

        for (level, zero_value) in zero_values.into_iter().enumerate() {
            let node_is_left_child = node_idx.is_multiple_of(2);
            siblings_path[level] = level_nodes.get(node_idx ^ 1).copied().unwrap_or(zero_value);
            siblings_side[level] = u8::from(node_is_left_child);

            level_nodes = level_nodes
                .chunks(2)
                .map(|pair| poseidon_hash(pair[0], pair.get(1).copied().unwrap_or(zero_value)))
                .collect();
            node_idx /= 2;
        }

        Self {
            siblings_path,
            siblings_side,
            root: level_nodes[0],
        }
    }
}

/// Off-chain copy of the program's on-chain `ImtTree`: same fields and insert rule, but the root is
/// returned by `insert` instead of stored. It keeps one node per level, so an insert costs `DEPTH`
/// hashes, but it can't give a Merkle path for an older leaf.
///
/// The circuit's `OffChainImt` builds the same tree (same leaves give the same root), but it
/// stores all `2^(DEPTH+1)` nodes and rehashes them all, so it can give a path for any leaf.
/// The paths here come from `MerklePath::from_leaves` instead.
#[derive(Clone, Copy)]
pub struct LocalImtTree<const DEPTH: usize> {
    /// Per level, the last left child on the path of the latest insert.
    frontiers: [Fr; DEPTH],
    zero_values: [Fr; DEPTH],
    next_leaf_idx: usize,
}

impl<const DEPTH: usize> LocalImtTree<DEPTH> {
    /// Leaves in insert order: replays each insert against its `new_root`, reordering within a
    /// slot.
    pub fn order_inserted_leaves(mut inserted_leaves: Vec<InsertedLeaf>) -> RequestResult<Vec<Fr>> {
        let mut local_imt_tree = Self::new();

        for next_leaf_idx in 0..inserted_leaves.len() {
            let slot = inserted_leaves[next_leaf_idx].slot;
            let remaining_leaves = &inserted_leaves[next_leaf_idx..];

            let (matching_offset, tree_after_insert) = local_imt_tree
                .find_next_leaf(remaining_leaves)
                .ok_or_else(|| {
                    AppError::Internal(format_err!(
                        "tree sync: no DepositDone of slot {slot} gives its new_root as leaf \
                         {next_leaf_idx}, the synced leaves are incomplete"
                    ))
                })?;

            inserted_leaves.swap(next_leaf_idx, next_leaf_idx + matching_offset);
            local_imt_tree = tree_after_insert;
        }

        Ok(inserted_leaves
            .into_iter()
            .map(|inserted_leaf| inserted_leaf.leaf)
            .collect())
    }

    fn new() -> Self {
        let zero_values: [Fr; DEPTH] = generate_zero_values_for_levels(DEPTH)
            .try_into()
            .expect("one zero value per level");

        Self {
            frontiers: zero_values,
            zero_values,
            next_leaf_idx: 0,
        }
    }

    /// The tree after `leaf` is inserted at the next free index, and its root.
    fn insert(&self, leaf: Fr) -> (Self, Fr) {
        let mut tree_after_insert = *self;
        let mut node_idx = self.next_leaf_idx;
        let mut node = leaf;

        for level in 0..DEPTH {
            node = if node_idx.is_multiple_of(2) {
                tree_after_insert.frontiers[level] = node;
                poseidon_hash(node, self.zero_values[level])
            } else {
                poseidon_hash(self.frontiers[level], node)
            };
            node_idx /= 2;
        }

        tree_after_insert.next_leaf_idx += 1;
        (tree_after_insert, node)
    }

    /// Offset of the first-slot leaf whose insert gives its `new_root`, and the tree after it.
    fn find_next_leaf(&self, remaining_leaves: &[InsertedLeaf]) -> Option<(usize, Self)> {
        let slot = remaining_leaves[0].slot;

        for (offset, inserted_leaf) in remaining_leaves.iter().enumerate() {
            if inserted_leaf.slot != slot {
                return None;
            }

            let (tree_after_insert, root_after_insert) = self.insert(inserted_leaf.leaf);
            if root_after_insert == inserted_leaf.new_root {
                return Some((offset, tree_after_insert));
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shielded_pool_circuit::{
        circuit::{
            constraints::poseidon::solana_poseidon_native::hash1,
            full_circuit::{deposit_commitment_hash, user_commitment_hash},
            prover::build_fixture_input_for_step,
        },
        imt::off_chain_imt::{MerkleProof, OffChainImt},
        transcript::fr_to_be,
    };

    use crate::zk::field::TREE_DEPTH;

    const OFF_CHAIN_IMT_DEPTH: usize = 3;
    const FIXTURE_STEP0_PUBLIC_INPUTS: &[u8; 160] =
        include_bytes!("../../tests/fixtures/step0/public_inputs.bin");

    fn create_leaves(leaf_count: u64) -> Vec<Fr> {
        (1..=leaf_count).map(hash1).collect()
    }

    /// One leaf per slot, each with the root after its insert, from the path builder.
    fn create_inserted_leaves(leaves: &[Fr]) -> Vec<InsertedLeaf> {
        (0..leaves.len())
            .map(|leaf_idx| InsertedLeaf {
                slot: leaf_idx as u64,
                leaf: leaves[leaf_idx],
                new_root: MerklePath::<TREE_DEPTH>::from_leaves(&leaves[..=leaf_idx], leaf_idx)
                    .root,
            })
            .collect()
    }

    #[test]
    fn merkle_path_matches_the_circuit_off_chain_imt() {
        let max_leaf_count = 1u64 << OFF_CHAIN_IMT_DEPTH;
        for leaf_count in 1..=max_leaf_count {
            let leaves = create_leaves(leaf_count);
            let mut off_chain_imt = OffChainImt::new(OFF_CHAIN_IMT_DEPTH as u32);
            for leaf in &leaves {
                off_chain_imt.insert_leaf_lazy(*leaf).unwrap();
            }
            off_chain_imt.build_tree();

            for (leaf_idx, leaf) in leaves.iter().enumerate() {
                let merkle_path = MerklePath::<OFF_CHAIN_IMT_DEPTH>::from_leaves(&leaves, leaf_idx);
                let off_chain_imt_proof = off_chain_imt.merkle_proof(*leaf).unwrap();

                assert_eq!(
                    merkle_path.root,
                    off_chain_imt.root(),
                    "{leaf_count} leaves"
                );
                assert_eq!(
                    merkle_path.siblings_path.to_vec(),
                    off_chain_imt_proof.siblings_path
                );
                assert_eq!(
                    merkle_path.siblings_side.to_vec(),
                    off_chain_imt_proof.siblings_side
                );
                assert!(off_chain_imt.verify_merkle_proof(&MerkleProof {
                    leaf: *leaf,
                    siblings_path: merkle_path.siblings_path.to_vec(),
                    siblings_side: merkle_path.siblings_side.to_vec(),
                }));
            }
        }
    }

    /// The fixture deposit as the only leaf of the depth-20 tree gives the fixture witness path and root.
    #[test]
    fn fixture_leaf_gives_the_fixture_path_and_root() {
        let fixture_input = build_fixture_input_for_step(0);

        let user_commitment_hash = user_commitment_hash(
            fixture_input.s,
            &fixture_input.chunks,
            &fixture_input.addresses,
        );

        let leaf = deposit_commitment_hash(user_commitment_hash, fixture_input.total_amount);

        let merkle_path = MerklePath::<TREE_DEPTH>::from_leaves(&[leaf], 0);

        assert_eq!(merkle_path.siblings_path, fixture_input.siblings_path);
        assert_eq!(merkle_path.siblings_side, fixture_input.siblings_side);
        assert_eq!(
            fr_to_be(&merkle_path.root),
            FIXTURE_STEP0_PUBLIC_INPUTS[128..160]
        );
    }

    #[test]
    fn replay_matches_the_path_builder_root_after_every_insert() {
        for leaf_count in 1..=10 {
            let leaves = create_leaves(leaf_count);

            let ordered_leaves =
                LocalImtTree::<TREE_DEPTH>::order_inserted_leaves(create_inserted_leaves(&leaves))
                    .unwrap();

            assert_eq!(ordered_leaves, leaves, "{leaf_count} leaves");
        }
    }

    /// The RPC may return the deposits of one slot in any order.
    #[test]
    fn replay_reorders_the_leaves_of_one_slot() {
        let leaves = create_leaves(4);
        let mut inserted_leaves = create_inserted_leaves(&leaves);
        for inserted_leaf in &mut inserted_leaves[1..] {
            inserted_leaf.slot = 1;
        }
        inserted_leaves[1..].reverse();

        let ordered_leaves =
            LocalImtTree::<TREE_DEPTH>::order_inserted_leaves(inserted_leaves).unwrap();

        assert_eq!(ordered_leaves, leaves);
    }

    /// Leaves of different slots in the wrong order, or a missing leaf, fail the sync.
    #[test]
    fn replay_rejects_a_wrong_order_or_a_missing_leaf() {
        let leaves = create_leaves(3);

        let mut swapped_across_slots = create_inserted_leaves(&leaves);
        swapped_across_slots.swap(0, 1);
        assert!(LocalImtTree::<TREE_DEPTH>::order_inserted_leaves(swapped_across_slots).is_err());

        let mut missing_leaf = create_inserted_leaves(&leaves);
        missing_leaf.remove(1);
        assert!(LocalImtTree::<TREE_DEPTH>::order_inserted_leaves(missing_leaf).is_err());
    }
}
