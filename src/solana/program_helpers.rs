//! Program interface generated from `idls/zk_shielded_pool_solana.json`.
//!
//! `declare_program!` generates the instruction arg structs (`instruction::*`), the account
//! lists (`accounts::*`), the account types, the events and the error enum. Nothing in this
//! crate hand-encodes instruction data or discriminators. Refresh the IDL with
//! `scripts/sync-idl.sh` after a program change.

use std::{collections::HashMap, sync::LazyLock};

use anchor_lang::prelude::*;
use solana_pubkey::Pubkey;

declare_program!(zk_shielded_pool_solana);

pub use zk_shielded_pool_solana::{
    ID, ProgramConfig, ProofStorage, PublicInputs, RootRegistry, accounts, events, instruction,
};

/// The IDL the module above is generated from. Also the source of `program_error_name`.
const IDL_JSON: &str = include_str!("../../idls/zk_shielded_pool_solana.json");

// PDA seeds, the same as the program's `SEED_PREFIX` constants (the IDL does not export them).
pub const PROGRAM_CONFIG_SEED: &[u8] = b"program_config";
pub const VAULT_SEED: &[u8] = b"vault";
pub const ROOT_REGISTRY_SEED: &[u8] = b"root_registry";
pub const PROOF_STORAGE_SEED: &[u8] = b"proof_storage";
pub const NULLIFIER_SEED: &[u8] = b"nullifier";

/// The 1088-byte proof does not fit one transaction: bytes `0..800` go in part 0, the rest in part 1.
pub const PROOF_UPLOAD_PART0_LEN: usize = 800;
/// `withdraw` runs the GWC verifier: it needs a raised CU limit and a bigger heap frame.
pub const WITHDRAW_CU_LIMIT: u32 = 1_400_000;
pub const WITHDRAW_HEAP_BYTES: u32 = 65_536;

/// Generate a map of program error codes to their names, from the IDL `errors` array. 
/// Global and built once, on first use. Thread safe.
static PROGRAM_ERROR_CODES_TO_NAMES: LazyLock<HashMap<u32, String>> = LazyLock::new(|| {
    let idl: serde_json::Value = serde_json::from_str(IDL_JSON).expect("bundled IDL is valid JSON");
    idl["errors"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|error| {
            let code = u32::try_from(error["code"].as_u64()?).ok()?;
            Some((code, error["name"].as_str()?.to_owned()))
        })
        .collect()
});

pub fn pda_program_config() -> Pubkey {
    find_pda(&[PROGRAM_CONFIG_SEED])
}

pub fn pda_vault() -> Pubkey {
    find_pda(&[VAULT_SEED])
}

pub fn pda_root_registry() -> Pubkey {
    find_pda(&[ROOT_REGISTRY_SEED])
}

/// Proof buffer of one `sender`. The same sender must upload, withdraw and close.
pub fn pda_proof(sender: &Pubkey, proof_hash: &[u8; 32]) -> Pubkey {
    find_pda(&[PROOF_STORAGE_SEED, sender.as_ref(), proof_hash])
}

/// `nullifier_be` is the big-endian nullifier, the same bytes as `public_inputs.nullifier`.
pub fn pda_nullifier(nullifier_be: &[u8; 32]) -> Pubkey {
    find_pda(&[NULLIFIER_SEED, nullifier_be])
}

fn find_pda(seeds: &[&[u8]]) -> Pubkey {
    Pubkey::find_program_address(seeds, &ID).0
}

/// Name of a program error code (`6000 + index` in the IDL `errors` array), e.g. `6009 -> UnknownRoot`.
///
/// The generated error enum only converts name -> code, so the reverse table is read from
/// the same IDL the enum comes from.
pub fn get_error_name(code: u32) -> Option<&'static str> {
    PROGRAM_ERROR_CODES_TO_NAMES.get(&code).map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::DEFAULT_PROGRAM_ID;

    #[test]
    fn idl_address_matches_default_program_id() {
        assert_eq!(ID, DEFAULT_PROGRAM_ID.parse::<Pubkey>().unwrap());
    }

    #[test]
    fn error_names_come_from_the_idl() {
        assert_eq!(get_error_name(6000), Some("TreeIsFull"));
        assert_eq!(get_error_name(6008), Some("InvalidProof"));
        assert_eq!(get_error_name(6009), Some("UnknownRoot"));
        assert_eq!(get_error_name(6011), Some("DestinationMismatch"));
        assert_eq!(get_error_name(6027), Some("ProgramPaused"));
        assert_eq!(get_error_name(6028), Some("DepositBelowMinimum"));
        assert_eq!(get_error_name(6029), None);
        assert_eq!(get_error_name(1), None);
    }

    #[test]
    fn pdas_use_the_program_seeds() {
        let (expected, _) = Pubkey::find_program_address(&[b"vault"], &ID);
        assert_eq!(pda_vault(), expected);
        let sender = Pubkey::new_unique();
        let hash = [7u8; 32];
        let (expected, _) =
            Pubkey::find_program_address(&[b"proof_storage", sender.as_ref(), &hash], &ID);
        assert_eq!(pda_proof(&sender, &hash), expected);
    }
}
