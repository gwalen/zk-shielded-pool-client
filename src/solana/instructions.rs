//! Instruction builders over the generated program interface.
//!
//! Same shape as `SessionKeysClient` in r3-session-keys: `program.request().accounts(..).args(..)`.
//! Builders are pure (no RPC, no signing). Signing, fee payment and sending happen in `solana::tx`.

use std::sync::Arc;

use anchor_client::{Client, Cluster, Program};
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;

use super::program_helpers::{
    ID, PublicInputs, accounts, instruction, pda_nullifier, pda_program_config, pda_proof,
    pda_root_registry, pda_vault,
};
use super::constants::RPC_COMMITMENT;
use crate::app_error::RawResult;

const SYSTEM_PROGRAM_ID: Pubkey = solana_system_interface::program::ID;

pub struct ZkPoolProgram {
    // `Arc<Keypair>` is the type of anchor-client's payer for the program
    pub program: Program<Arc<Keypair>>,
}

impl ZkPoolProgram {
    // Payer (payer_placeholder) is a throwaway keypair: Client::new_with_options needs one, but we only
    // call .instructions(), never .send(). Signing and fee payment happen in solana::tx_builder
    pub fn new(cluster: Cluster) -> RawResult<Self> {
        let payer_placeholder = Keypair::new();
        let client = Client::new_with_options(cluster, Arc::new(payer_placeholder), RPC_COMMITMENT);
        let program = client
            .program(ID)
            .map_err(|e| anyhow::anyhow!("anchor client: {e}"))?; // -- next is this error creation which I dont like
        Ok(Self { program })
    }

    /// `user_commitment_hash` is the LE bytes of the user commitment (`zk::field::fr_le`).
    pub fn deposit(&self, sender: Pubkey, user_commitment_hash: [u8; 32], total_amount: u64) -> Instruction {
        self.program
            .request()
            .accounts(accounts::Deposit {
                sender,
                program_config: pda_program_config(),
                vault: pda_vault(),
                roots_registry: pda_root_registry(),
                system_program: SYSTEM_PROGRAM_ID,
            })
            .args(instruction::Deposit { user_commitment_hash, total_amount })
            .instructions()
            .remove(0)
    }

    pub fn upload_proof(
        &self,
        sender: Pubkey,
        proof_hash: [u8; 32],
        part: u8,
        proof_final_len: u16,
        proof: Vec<u8>,
    ) -> Instruction {
        self.program
            .request()
            .accounts(accounts::UploadProof {
                sender,
                program_config: pda_program_config(),
                proof_account: pda_proof(&sender, &proof_hash),
                system_program: SYSTEM_PROGRAM_ID,
            })
            .args(instruction::UploadProof { _proof_hash: proof_hash, part, proof_final_len, proof })
            .instructions()
            .remove(0)
    }

    /// The proof PDA is seeded with `sender`, so the same wallet must have uploaded the proof.
    /// The nullifier PDA comes from `public_inputs.nullifier` (BE).
    pub fn withdraw(
        &self,
        sender: Pubkey,
        recipient: Pubkey,
        proof_hash: [u8; 32],
        public_inputs: PublicInputs,
    ) -> Instruction {
        let nullifier_account = pda_nullifier(&public_inputs.nullifier);
        self.program
            .request()
            .accounts(accounts::Withdraw {
                sender,
                program_config: pda_program_config(),
                vault: pda_vault(),
                roots_registry: pda_root_registry(),
                proof_account: pda_proof(&sender, &proof_hash),
                recipient,
                nullifier_account,
                system_program: SYSTEM_PROGRAM_ID,
            })
            .args(instruction::Withdraw { proof_hash, public_inputs })
            .instructions()
            .remove(0)
    }

    pub fn close_proof(&self, sender: Pubkey, proof_hash: [u8; 32]) -> Instruction {
        self.program
            .request()
            .accounts(accounts::CloseProof { sender, proof_account: pda_proof(&sender, &proof_hash) })
            .args(instruction::CloseProof { _proof_hash: proof_hash })
            .instructions()
            .remove(0)
    }
}

/// [SetComputeUnitLimit, RequestHeapFrame], put it before withdraw ix in the same tx
pub fn compute_budget_ixs(cu_limit: u32, heap_bytes: u32) -> [Instruction; 2] {
    [
        ComputeBudgetInstruction::set_compute_unit_limit(cu_limit),
        ComputeBudgetInstruction::request_heap_frame(heap_bytes),
    ]
}
