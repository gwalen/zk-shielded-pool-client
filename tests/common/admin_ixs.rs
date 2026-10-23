//! Admin instructions the server never sends: `initialize` (once per deployment) and
//! `pause` / `unpause` (owner only). Tests send them to set up the chain themselves.

use solana_instruction::Instruction;
use solana_pubkey::Pubkey;
use zk_shielded_pool_client::solana::{
    instructions::ZkPoolProgram,
    program_helpers::{accounts, instruction, pda_program_config, pda_root_registry, pda_vault},
};

pub fn initialize(pool: &ZkPoolProgram, signer: Pubkey) -> Instruction {
    pool.program
        .request()
        .accounts(accounts::Initialize {
            signer,
            program_config: pda_program_config(),
            vault: pda_vault(),
            root_registry: pda_root_registry(),
            system_program: solana_system_interface::program::ID,
        })
        .args(instruction::Initialize {})
        .instructions()
        .remove(0)
}

pub fn pause(pool: &ZkPoolProgram, owner: Pubkey) -> Instruction {
    pool.program
        .request()
        .accounts(accounts::Pause { owner, program_config: pda_program_config() })
        .args(instruction::Pause {})
        .instructions()
        .remove(0)
}

pub fn unpause(pool: &ZkPoolProgram, owner: Pubkey) -> Instruction {
    pool.program
        .request()
        .accounts(accounts::Unpause { owner, program_config: pda_program_config() })
        .args(instruction::Unpause {})
        .instructions()
        .remove(0)
}
