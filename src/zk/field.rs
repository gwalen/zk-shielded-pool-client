//! Field helpers: thin wrappers over the circuit crate. Poseidon is never re-implemented here.
//!
//! Byte order: tree nodes, Poseidon in/out, on-chain storage and the deposit ix use **LE**
//! (`fr_le`). Withdraw public inputs and the nullifier PDA seed use **BE** (`fr_be`).

use ff::PrimeField;
use shielded_pool_circuit::circuit::{
    constraints::poseidon::solana_poseidon_native::hash2, full_circuit, utils,
};
use shielded_pool_circuit::imt::imt_utils::{generate_zero_values_for_levels, poseidon_hash};
use shielded_pool_circuit::transcript;
use solana_pubkey::Pubkey;

pub use shielded_pool_circuit::Fr;

/// Number of chunks (and destinations) per deposit.
pub const CHUNKS: usize = shielded_pool_circuit::circuit::consts::MAX_CHUNKS;
/// Depth of the on-chain tree (`MERKLE_TREE_DEPTH`) and of the production circuit.
pub const TREE_DEPTH: usize = shielded_pool_circuit::circuit::consts::PROD_TREE_DEPTH;

/// Destination address as a field element: Poseidon over the 4 u64 limbs of the pubkey bytes.
pub fn pubkey_to_fr(pubkey: &Pubkey) -> Fr {
    utils::convert_pubkey_32bytes_to_fr(pubkey.to_bytes())
}

/// `Poseidon(s, c0, c1, c2, A0, A1, A2)`: the value sent in the deposit ix (as LE bytes).
pub fn user_commitment(s: Fr, chunks: &[Fr; CHUNKS], addresses: &[Fr; CHUNKS]) -> Fr {
    full_circuit::user_commitment_hash(s, chunks, addresses)
}

/// `Poseidon(user_commitment, total)`: the tree leaf the program inserts.
pub fn deposit_commitment(user_commitment: Fr, total_amount: Fr) -> Fr {
    full_circuit::deposit_commitment_hash(user_commitment, total_amount)
}

/// `Poseidon(s, step)`: one nullifier per chunk.
pub fn nullifier(s: Fr, step: u8) -> Fr {
    hash2(&[s, Fr::from(u64::from(step))])
}

/// Root of the empty depth-20 tree, what `initialize` stores: `Z_20 = H(Z_19, Z_19)`.
pub fn empty_tree_root() -> Fr {
    let zero_values = generate_zero_values_for_levels(TREE_DEPTH);
    let top = zero_values[TREE_DEPTH - 1];
    poseidon_hash(top, top)
}

pub fn fr_le(value: Fr) -> [u8; 32] {
    utils::fr_to_le_bytes(value)
}

pub fn fr_be(value: Fr) -> [u8; 32] {
    transcript::fr_to_be(&value)
}

/// `None` if the bytes are not a canonical field element (>= modulus).
pub fn fr_from_le(bytes: [u8; 32]) -> Option<Fr> {
    Option::from(Fr::from_repr(bytes.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ff::Field;

    const FIXTURE_PUBLIC_INPUTS: &[u8; 160] = include_bytes!("../../tests/fixtures/step0/public_inputs.bin");
    const DST: &str = "dstH17g8RBGdUo3YeYhSFHDdFzHrWkAzNCKSveAchyD";

    fn fixture_public_input(index: usize) -> [u8; 32] {
        FIXTURE_PUBLIC_INPUTS[index * 32..(index + 1) * 32].try_into().unwrap()
    }

    /// Reference vector from the program's `dest_address_hash.rs` test.
    #[test]
    fn pubkey_to_fr_matches_the_program() {
        let fr = pubkey_to_fr(&DST.parse().unwrap());
        assert_eq!(
            hex::encode(fr_le(fr)),
            "c2332be1ab10bbdb95a6794fd868716df5c6a979b237b4f7d395f5e568f7fe2e"
        );
        assert_eq!(fr_be(fr), fixture_public_input(2), "dest_address public input is BE");
    }

    /// The fixture deposit as the only leaf of a depth-20 tree gives the fixture root.
    #[test]
    fn fixture_commitment_gives_the_fixture_root() {
        let s = Fr::from(1_234_567_890u64);
        let dst = pubkey_to_fr(&DST.parse().unwrap());
        let chunks = [2_000_000_000u64, 3_000_000_000, 4_000_000_000].map(Fr::from);
        let leaf = deposit_commitment(user_commitment(s, &chunks, &[dst; CHUNKS]), Fr::from(9_000_000_000u64));

        // Leaf index 0: every sibling is on the right and is the zero value of its level.
        let root = generate_zero_values_for_levels(TREE_DEPTH)
            .into_iter()
            .fold(leaf, poseidon_hash);

        assert_eq!(fr_be(root), fixture_public_input(4));
        assert_eq!(fr_be(nullifier(s, 0)), fixture_public_input(3));
    }

    #[test]
    fn empty_root_is_the_zero_leaf_hashed_to_the_top() {
        let folded = generate_zero_values_for_levels(TREE_DEPTH)
            .into_iter()
            .fold(Fr::ZERO, poseidon_hash);
        assert_eq!(empty_tree_root(), folded);
    }

    #[test]
    fn le_and_be_are_reverses_and_round_trip() {
        let value = Fr::from(0x0102_0304u64);
        let le = fr_le(value);
        assert_eq!(&le[..4], &[4, 3, 2, 1]);
        let mut be = fr_be(value);
        be.reverse();
        assert_eq!(be, le);
        assert_eq!(fr_from_le(le), Some(value));
        assert_eq!(fr_from_le([0xFF; 32]), None, "above the modulus");
    }
}
