//! Program-level tests without a validator: load the built `.so` into LiteSVM and run the
//! V0 transactions built by `ZkPoolProgram` + `solana::tx::compile_v0`.
//!
//! Pattern from r3-session-keys `tests/common/{env,mod}.rs`. The `.so` path comes from
//! `ZK_POOL_SO` (default `../pool-anchor-v2-rewrite/target/deploy/zk_shielded_pool_solana.so`).
//! If it is missing, each test prints how to build it and returns.

#[path = "../common/mod.rs"]
mod common;

use std::{array, env, fs, path::PathBuf};

use anchor_client::Cluster;
use common::admin_ixs;
use litesvm::{
    LiteSVM,
    types::{FailedTransactionMetadata, TransactionMetadata},
};
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use shielded_pool_circuit::{circuit::{
    full_circuit::{deposit_commitment_hash, user_commitment_hash},
    utils::{convert_pubkey_32bytes_to_fr, fr_to_le_bytes},
}, imt::imt_utils};
use solana_signer::Signer;
use zk_shielded_pool_client::{
    solana::{
        accounts::{ProgramConfigView, RootRegistryView},
        events::parse_deposit_done_events,
        instructions::ZkPoolProgram,
        program_helpers::{ID, derive_program_config_pda, derive_root_registry_pda, derive_vault_pda},
        tx_builder::{self, compile_v0},
    },
    zk::field::{CHUNKS, Fr, TREE_DEPTH, create_empty_tree_root},
};

const DEFAULT_SO: &str = "../pool-anchor-v2-rewrite/target/deploy/zk_shielded_pool_solana.so";
const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

struct Env {
    svm: LiteSVM,
    pool: ZkPoolProgram,
    payer: Keypair,
}

impl Env {
    /// `None` (and a message) if the program binary is not built.
    fn new() -> Option<Self> {
        let path = env::var("ZK_POOL_SO")
            .map(PathBuf::from)
            .unwrap_or_else(|_| DEFAULT_SO.into());
        let Ok(program) = fs::read(&path) else {
            eprintln!(
                "SKIPPED: program binary not found at {}. Build it with `just program-build` \
                 (or set ZK_POOL_SO).",
                path.display()
            );
            return None;
        };

        let mut svm = LiteSVM::new();
        svm.add_program(ID, &program).unwrap();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 100 * LAMPORTS_PER_SOL)
            .unwrap();
        let pool = ZkPoolProgram::new(Cluster::Localnet).unwrap();
        Some(Self { svm, pool, payer })
    }

    #[allow(clippy::result_large_err)] // LiteSVM's own failure type
    fn send(
        &mut self,
        signer: &Keypair,
        ixs: &[Instruction],
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let tx = compile_v0(signer, ixs, self.svm.latest_blockhash()).unwrap();
        let result = self.svm.send_transaction(tx);
        // LiteSVM does not produce blocks: expire the blockhash so the next tx gets a new one.
        self.svm.expire_blockhash();
        result
    }

    fn send_ok(&mut self, signer: &Keypair, ixs: &[Instruction]) -> TransactionMetadata {
        self.send(signer, ixs).unwrap_or_else(|failure| {
            panic!(
                "transaction failed: {:?}\nlogs:\n{}",
                failure.err,
                failure.meta.logs.join("\n")
            )
        })
    }

    fn initialize(&mut self) {
        let payer = self.payer.insecure_clone();
        let ix = admin_ixs::initialize(&self.pool, payer.pubkey());
        self.send_ok(&payer, &[ix]);
    }

    fn account_data(&self, address: &Pubkey) -> Vec<u8> {
        self.svm.get_account(address).expect("account exists").data
    }

    fn fetch_root_registry_account(&self) -> RootRegistryView {
        RootRegistryView::decode(&self.account_data(&derive_root_registry_pda())).unwrap()
    }
}

#[test]
fn initialize_creates_an_empty_tree() {
    let Some(mut env) = Env::new() else { return };
    env.initialize();

    let program_config_account_view =
        ProgramConfigView::decode(&env.account_data(&derive_program_config_pda())).unwrap();
    assert_eq!(program_config_account_view.owner, env.payer.pubkey());
    assert!(!program_config_account_view.paused);
    assert!(program_config_account_view.min_deposit_lamports > 0);

    let root_registry_account_view = env.fetch_root_registry_account();
    assert_eq!(root_registry_account_view.next_leaf_idx, 0);
    assert_eq!(root_registry_account_view.root_le, fr_to_le_bytes(create_empty_tree_root()));
    assert!(root_registry_account_view.is_known_root(&root_registry_account_view.root_le));
}

#[test]
fn deposit_emits_the_locally_computed_commitment() {
    let Some(mut env) = Env::new() else { return };
    env.initialize();

    let depositor = Keypair::new();
    env.svm
        .airdrop(&depositor.pubkey(), 20 * LAMPORTS_PER_SOL)
        .unwrap();

    let s = Fr::from(1_234_567_890u64);
    let chunks = [2, 3, 4].map(|sol: u64| sol * LAMPORTS_PER_SOL);
    let destinations: [Fr; CHUNKS] = array::from_fn(|_| convert_pubkey_32bytes_to_fr(Pubkey::new_unique().to_bytes()));
    let total: u64 = chunks.iter().sum();

    let user_commitment_hash = user_commitment_hash(s, &chunks.map(Fr::from), &destinations);
    let expected_leaf = deposit_commitment_hash(user_commitment_hash, Fr::from(total));

    let vault_balance_before = env.svm.get_balance(&derive_vault_pda()).unwrap_or(0);

    let ix = env.pool.deposit(depositor.pubkey(), fr_to_le_bytes(user_commitment_hash), total);
    let meta = env.send_ok(&depositor, &[ix]);

    let events = parse_deposit_done_events(&meta.logs).unwrap();
    assert_eq!(events.len(), 1, "logs:\n{}", meta.logs.join("\n"));
    let event = &events[0];
    assert_eq!(event.user_commitment_hash.to_bytes(), fr_to_le_bytes(user_commitment_hash));
    assert_eq!(event.total_amount, total);
    assert_eq!(
        event.deposit_commitment_hash.to_bytes(),
        fr_to_le_bytes(expected_leaf),
        "client and program hash differently"
    );

    // Single leaf at index 0: the root is the leaf hashed with each level's zero value.
    let expected_root =
        imt_utils::generate_zero_values_for_levels(TREE_DEPTH)
            .into_iter()
            .fold(
                expected_leaf,
                imt_utils::poseidon_hash,
            );
    
    let root_registry_account_view = env.fetch_root_registry_account();
    assert_eq!(root_registry_account_view.next_leaf_idx, 1);
    assert_eq!(root_registry_account_view.root_le, fr_to_le_bytes(expected_root));
    assert_eq!(
        event.new_root.to_bytes(),
        root_registry_account_view.root_le
    );
    assert!(root_registry_account_view.is_known_root(&root_registry_account_view.root_le));
    assert_eq!(
        env.svm.get_balance(&derive_vault_pda()).unwrap(),
        vault_balance_before + total
    );
}

#[test]
fn deposit_below_minimum_names_the_program_error() {
    let Some(mut env) = Env::new() else { return };
    env.initialize();

    let user_commitment_hash = user_commitment_hash(
        Fr::from(1u64),
        &[Fr::from(1u64); CHUNKS],
        &[Fr::from(2u64); CHUNKS],
    );
    let payer = env.payer.insecure_clone();
    let ix = env.pool.deposit(payer.pubkey(), fr_to_le_bytes(user_commitment_hash), 1);
    let failure = env
        .send(&payer, &[ix])
        .expect_err("1 lamport is below the minimum");

    let described =
        tx_builder::parse_transaction_error(&failure.err);
    assert!(described.contains("DepositBelowMinimum"), "{described}");
}
