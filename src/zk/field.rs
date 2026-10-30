//! Field rules on top of the circuit crate. Poseidon is never re-implemented here.
//!
//! Byte order: tree nodes, Poseidon in/out, on-chain storage and the deposit ix use **LE**
//! (`utils::fr_to_le_bytes`). Withdraw public inputs and the nullifier PDA seed use **BE**
//! (`transcript::fr_to_be`).

use ff::PrimeField;
use shielded_pool_circuit::circuit::constraints::poseidon::solana_poseidon_native::hash2;
use shielded_pool_circuit::circuit::consts;
use shielded_pool_circuit::imt::imt_utils::{generate_zero_values_for_levels, poseidon_hash};

pub use shielded_pool_circuit::Fr;

/// Number of chunks (and destinations) per deposit.
pub const CHUNKS: usize = consts::MAX_CHUNKS;
/// Depth of the on-chain tree (`MERKLE_TREE_DEPTH`) and of the production circuit.
pub const TREE_DEPTH: usize = consts::PROD_TREE_DEPTH;

/// `Poseidon(s, step)`: one nullifier per chunk.
pub fn create_nullifier(s: Fr, step: u8) -> Fr {
    hash2(&[s, Fr::from(u64::from(step))])
}

/// Root of the empty depth-20 tree, what `initialize` stores: `Z_20 = H(Z_19, Z_19)`.
pub fn create_empty_tree_root() -> Fr {
    let zero_values = generate_zero_values_for_levels(TREE_DEPTH);
    let top = zero_values[TREE_DEPTH - 1];
    poseidon_hash(top, top)
}

/// `None` if the bytes are not a canonical field element (>= modulus).
pub fn fr_from_le(bytes: [u8; 32]) -> Option<Fr> {
    Option::from(Fr::from_repr(bytes.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ff::Field;
    use shielded_pool_circuit::circuit::{
        full_circuit::{deposit_commitment_hash, user_commitment_hash},
        utils::{convert_pubkey_32bytes_to_fr, fr_to_le_bytes},
    };
    use shielded_pool_circuit::transcript::fr_to_be;
    use solana_pubkey::Pubkey;

    const FIXTURE_PUBLIC_INPUTS: &[u8; 160] = include_bytes!("../../tests/fixtures/step0/public_inputs.bin");
    const DESTINATION: &str = "dstH17g8RBGdUo3YeYhSFHDdFzHrWkAzNCKSveAchyD";

    fn fixture_public_input(index: usize) -> [u8; 32] {
        FIXTURE_PUBLIC_INPUTS[index * 32..(index + 1) * 32].try_into().unwrap()
    }

    /// Reference vector from the program's `dest_address_hash.rs` test.
    #[test]
    fn destination_hash_matches_the_program() {
        let destination: Pubkey = DESTINATION.parse().unwrap();
        let destination_fr = convert_pubkey_32bytes_to_fr(destination.to_bytes());
        assert_eq!(
            hex::encode(fr_to_le_bytes(destination_fr)),
            "c2332be1ab10bbdb95a6794fd868716df5c6a979b237b4f7d395f5e568f7fe2e"
        );
        assert_eq!(fr_to_be(&destination_fr), fixture_public_input(2), "dest_address public input is BE");
    }

    /// The fixture deposit as the only leaf of a depth-20 tree gives the fixture root.
    #[test]
    fn fixture_commitment_gives_the_fixture_root() {
        let s = Fr::from(1_234_567_890u64);
        let destination: Pubkey = DESTINATION.parse().unwrap();
        let destination_fr = convert_pubkey_32bytes_to_fr(destination.to_bytes());
        let chunks = [2_000_000_000u64, 3_000_000_000, 4_000_000_000].map(Fr::from);
        let user_commitment_hash = user_commitment_hash(s, &chunks, &[destination_fr; CHUNKS]);
        let leaf = deposit_commitment_hash(user_commitment_hash, Fr::from(9_000_000_000u64));

        // Leaf index 0: every sibling is on the right and is the zero value of its level.
        let root = generate_zero_values_for_levels(TREE_DEPTH)
            .into_iter()
            .fold(leaf, poseidon_hash);

        assert_eq!(fr_to_be(&root), fixture_public_input(4));
        assert_eq!(fr_to_be(&create_nullifier(s, 0)), fixture_public_input(3));
    }

    #[test]
    fn empty_root_is_the_zero_leaf_hashed_to_the_top() {
        let folded = generate_zero_values_for_levels(TREE_DEPTH)
            .into_iter()
            .fold(Fr::ZERO, poseidon_hash);
        assert_eq!(create_empty_tree_root(), folded);
    }

    #[test]
    fn le_and_be_are_reverses_and_round_trip() {
        let value = Fr::from(0x0102_0304u64);
        let value_le = fr_to_le_bytes(value);
        assert_eq!(&value_le[..4], &[4, 3, 2, 1]);
        let mut value_be_reversed = fr_to_be(&value);
        value_be_reversed.reverse();
        assert_eq!(value_be_reversed, value_le);
        assert_eq!(fr_from_le(value_le), Some(value));
        assert_eq!(fr_from_le([0xFF; 32]), None, "above the modulus");
    }
}
