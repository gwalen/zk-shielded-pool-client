use std::{sync::Arc, time::Instant};

use anyhow::format_err;
use chrono::Utc;
use sha3::{Digest, Keccak256};
use shielded_pool_circuit::{
    circuit::{
        prover::ProverInput,
        utils::{convert_pubkey_32bytes_to_fr, fr_to_le_bytes},
    },
    transcript::fr_to_be,
};
use solana_instruction::error::InstructionError;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_signature::Signature;
use solana_signer::Signer;
use solana_transaction_error::TransactionError;
use tokio::task::spawn_blocking;
use tracing::{error, info, warn};

use crate::{
    app_error::{AppError, RequestResult},
    solana::{
        accounts_helpers::{fetch_root_registry, fetch_withdrawal_accounts},
        events::fetch_deposit_done_events,
        instructions::compute_budget_ixs,
        program_helpers::{
            PROOF_UPLOAD_PART0_LEN, PublicInputs, WITHDRAW_CU_LIMIT, WITHDRAW_HEAP_BYTES,
            ZkShieldedPoolSolanaError, derive_nullifier_pda, derive_proof_pda,
        },
        tx_builder::{compile_v0, create_rpc_error, send_v0},
    },
    state::AppState,
    zk::{
        field::{CHUNKS, Fr, TREE_DEPTH, create_nullifier},
        merkle_tree::{InsertedLeaf, LocalImtTree, MerklePath},
        prover::{GeneratedProof, PUBLIC_INPUTS_LEN},
        zk_deposit_data::{WithdrawalRecord, ZkDepositData, ZkDepositStatus, parse_fr_hex},
    },
};

/// A deposit landing during the tree sync makes the sync run once more.
const TREE_SYNC_ATTEMPTS: usize = 2;

/// A root leaving the on-chain root history before the withdraw tx runs makes the withdrawal run once more.
const WITHDRAW_ATTEMPTS: usize = 2;

/// A withdraw request whose step and fee payer are already checked.
pub struct WithdrawRequest {
    pub deposit_id: String,
    /// Chunk index, below `CHUNKS`.
    pub step: u8,
    /// Configured withdraw wallet: signer and fee payer of upload_proof, withdraw and close_proof.
    pub fee_payer: Arc<Keypair>,
}

pub struct WithdrawReceipt {
    pub withdraw_tx: Signature,
    /// Part 0, then part 1.
    pub upload_txs: [Signature; 2],
    /// `None` when closing the proof account failed.
    pub close_proof_tx: Option<Signature>,
    pub recipient: Pubkey,
    pub amount: u64,
    pub nullifier_be: [u8; 32],
}

/// The values of one withdrawal step, read from the stored zk deposit data.
struct WithdrawalWitness {
    secret_s: Fr,
    deposit_commitment: Fr,
    total_amount: u64,
    chunks: [u64; CHUNKS],
    destinations: [Pubkey; CHUNKS],
    step: u8,
    nullifier: Fr,
    /// The nullifier PDA seed and `public_inputs.nullifier`.
    nullifier_be: [u8; 32],
}

/// The txs of a confirmed withdrawal.
struct WithdrawalTxs {
    /// Part 0, then part 1.
    upload_txs: [Signature; 2],
    withdraw_tx: Signature,
}

impl WithdrawalWitness {
    fn parse(zk_deposit_data: &ZkDepositData, step: u8) -> RequestResult<Self> {
        let secret_s = zk_deposit_data.secret()?;

        let deposit_commitment = parse_fr_hex(&zk_deposit_data.deposit_commitment_hash)
            .ok_or_else(|| {
                AppError::Internal(format_err!(
                    "zk deposit data {}: invalid deposit_commitment_hash",
                    zk_deposit_data.id
                ))
            })?;

        let mut destinations = [Pubkey::default(); CHUNKS];
        for (destination, stored_destination) in
            destinations.iter_mut().zip(&zk_deposit_data.destinations)
        {
            *destination = stored_destination.parse().map_err(|_| {
                AppError::Internal(format_err!(
                    "zk deposit data {}: invalid destination {stored_destination}",
                    zk_deposit_data.id
                ))
            })?;
        }

        let nullifier = create_nullifier(secret_s, step);

        Ok(Self {
            secret_s,
            deposit_commitment,
            total_amount: zk_deposit_data.total_amount,
            chunks: zk_deposit_data.chunks,
            destinations,
            step,
            nullifier,
            nullifier_be: fr_to_be(&nullifier),
        })
    }

    fn chunk_amount(&self) -> u64 {
        self.chunks[usize::from(self.step)]
    }

    fn recipient(&self) -> Pubkey {
        self.destinations[usize::from(self.step)]
    }

    fn create_prover_input(&self, merkle_path: &MerklePath<TREE_DEPTH>) -> ProverInput<TREE_DEPTH> {
        let addresses = self
            .destinations
            .map(|destination| convert_pubkey_32bytes_to_fr(destination.to_bytes()));

        ProverInput {
            s: self.secret_s,
            total_amount: Fr::from(self.total_amount),
            chunks: self.chunks.map(Fr::from),
            addresses,
            siblings_path: merkle_path.siblings_path,
            siblings_side: merkle_path.siblings_side,
            step: Fr::from(u64::from(self.step)),
            chunk_amount: Fr::from(self.chunk_amount()),
            dest_address: addresses[usize::from(self.step)],
            nullifier: self.nullifier,
            root: merkle_path.root,
        }
    }
}

/// Proves and pays out one chunk under the deposit lock; records it once the withdraw tx is confirmed.
pub async fn withdraw(
    state: &AppState,
    request: WithdrawRequest,
) -> RequestResult<WithdrawReceipt> {
    let _deposit_guard = state.withdraw_locks.lock(&request.deposit_id).await;

    let zk_deposit_data = state
        .zk_deposit_data_writer
        .get(&request.deposit_id)
        .await?;

    check_step_is_withdrawable(&zk_deposit_data, request.step)?;

    let withdrawal_witness = WithdrawalWitness::parse(&zk_deposit_data, request.step)?;
    let nullifier_pda = derive_nullifier_pda(&withdrawal_witness.nullifier_be);

    let withdrawal_accounts = fetch_withdrawal_accounts(&state.rpc, &nullifier_pda).await?;

    if withdrawal_accounts.nullifier_spent {
        // record the failed withdrawal attempt - no withdraw tx was sent
        record_withdrawal(
            state,
            &zk_deposit_data.id,
            request.step,
            None,
            &withdrawal_witness.nullifier_be,
        )
        .await?;
        return Err(AppError::Conflict(format!(
            "step {} of deposit {} was already withdrawn: its nullifier is spent on chain",
            request.step, zk_deposit_data.id
        )));
    }

    let program_config_account_view = withdrawal_accounts
        .program_config_account_view
        .as_ref()
        .ok_or_else(|| AppError::Internal(format_err!("the pool is not initialized")))?;

    if program_config_account_view.paused {
        return Err(AppError::BadRequest(
            "the pool is paused, withdrawals are disabled".into(),
        ));
    }

    prove_and_withdraw(state, &request, &withdrawal_witness).await
}

/// Syncs the tree, proves, uploads and withdraws; runs once more when the proof's root left the root history.
async fn prove_and_withdraw(
    state: &AppState,
    request: &WithdrawRequest,
    withdrawal_witness: &WithdrawalWitness,
) -> RequestResult<WithdrawReceipt> {
    let mut attempt = 1;
    loop {
        let merkle_path =
            sync_merkle_path(&state.rpc, withdrawal_witness.deposit_commitment).await?;

        let generated_proof =
            generate_proof(state, withdrawal_witness.create_prover_input(&merkle_path)).await?;

        check_public_inputs(
            &generated_proof.public_inputs_be,
            withdrawal_witness,
            &merkle_path,
        )?;

        let proof_hash: [u8; 32] = Keccak256::digest(&generated_proof.proof).into();

        let withdrawal_result = upload_proof_and_withdraw(
            state,
            request,
            withdrawal_witness,
            proof_hash,
            generated_proof,
        )
        .await;

        // The proof account may hold rent after any outcome.
        let close_proof_tx = close_proof(state, &request.fee_payer, proof_hash).await;

        match withdrawal_result {
            Ok(withdrawal_txs) => {
                return Ok(WithdrawReceipt {
                    withdraw_tx: withdrawal_txs.withdraw_tx,
                    upload_txs: withdrawal_txs.upload_txs,
                    close_proof_tx,
                    recipient: withdrawal_witness.recipient(),
                    amount: withdrawal_witness.chunk_amount(),
                    nullifier_be: withdrawal_witness.nullifier_be,
                });
            }
            Err(withdrawal_error) => {
                let root_left_history = is_unknown_root_rejection(&withdrawal_error);

                if !root_left_history || attempt == WITHDRAW_ATTEMPTS {
                    return Err(withdrawal_error);
                }

                warn!(deposit_id = %request.deposit_id, step = request.step, "The proof's root left the on-chain root history, proving again");
                attempt += 1;
            }
        }
    }
}

/// The deposit is confirmed, and the step has a chunk and no withdrawal yet.
fn check_step_is_withdrawable(zk_deposit_data: &ZkDepositData, step: u8) -> RequestResult<()> {
    if zk_deposit_data.status != ZkDepositStatus::Confirmed {
        return Err(AppError::Conflict(format!(
            "deposit {id} is {status:?}, only a Confirmed deposit can be withdrawn: \
             check it with POST /deposit/{id}/reconcile",
            id = zk_deposit_data.id,
            status = zk_deposit_data.status,
        )));
    }

    let step_idx = usize::from(step);

    if zk_deposit_data.chunks[step_idx] == 0 {
        return Err(AppError::BadRequest(format!(
            "step {step} of deposit {} has a zero chunk, there is nothing to withdraw",
            zk_deposit_data.id
        )));
    }

    if zk_deposit_data.withdrawals[step_idx].is_some() {
        return Err(AppError::Conflict(format!(
            "step {step} of deposit {} was already withdrawn",
            zk_deposit_data.id
        )));
    }

    Ok(())
}

/// Deposit commitment path in a synced tree that holds every on-chain leaf and a known root.
async fn sync_merkle_path(
    rpc: &RpcClient,
    deposit_commitment: Fr,
) -> RequestResult<MerklePath<TREE_DEPTH>> {
    let mut attempt = 1;
    loop {
        let (synced_leaf_count, merkle_path) = fetch_merkle_path(rpc, deposit_commitment).await?;

        let root_registry_account_view = fetch_root_registry(rpc)
            .await?
            .ok_or_else(|| AppError::Internal(format_err!("the pool is not initialized")))?;

        let next_leaf_idx = root_registry_account_view.next_leaf_idx as usize;

        let root_is_known =
            root_registry_account_view.is_known_root(&fr_to_le_bytes(merkle_path.root));

        if next_leaf_idx == synced_leaf_count && root_is_known {
            return Ok(merkle_path);
        }

        if attempt == TREE_SYNC_ATTEMPTS {
            return Err(AppError::Internal(format_err!(
                "tree sync: {synced_leaf_count} leaves synced, the root registry holds \
                 {next_leaf_idx}, synced root known on chain: {root_is_known}"
            )));
        }

        warn!(
            synced_leaf_count,
            next_leaf_idx, "A deposit landed during the tree sync, syncing again"
        );
        attempt += 1;
    }
}

/// The number of synced leaves and the deposit commitment's path among them.
async fn fetch_merkle_path(
    rpc: &RpcClient,
    deposit_commitment: Fr,
) -> RequestResult<(usize, MerklePath<TREE_DEPTH>)> {
    let inserted_leaves = fetch_deposit_done_events(rpc)
        .await?
        .iter()
        .map(|(slot, deposit_done)| InsertedLeaf::from_deposit_done(*slot, deposit_done))
        .collect::<RequestResult<Vec<_>>>()?;

    let leaves = LocalImtTree::<TREE_DEPTH>::order_inserted_leaves(inserted_leaves)?;

    let leaf_idx = leaves
        .iter()
        .position(|leaf| *leaf == deposit_commitment)
        .ok_or_else(|| {
            AppError::Internal(format_err!(
                "tree sync: the deposit commitment is not among the {} synced leaves",
                leaves.len()
            ))
        })?;

    Ok((
        leaves.len(),
        MerklePath::<TREE_DEPTH>::from_leaves(&leaves, leaf_idx),
    ))
}

/// One proof at a time, off the async runtime.
async fn generate_proof(
    state: &AppState,
    prover_input: ProverInput<TREE_DEPTH>,
) -> RequestResult<GeneratedProof> {
    // One proof at a time: halo2 already uses every core through rayon, so two proofs at once
    // share the cores and both finish at ~12 s, instead of ~6 s and ~12 s one after the other.
    // The wait is async, so a queued request doesn't hold a tokio worker.
    let _prover_permit = state
        .prover_permits
        .acquire()
        .await
        .expect("the prover semaphore was never closed");

    let prover = Arc::clone(&state.prover);
    let proving_started = Instant::now();

    // Long running tasks like proof generation should run on a separate thread pool, so
    // spawn_blocking moves it off the server's tokio thread pool. If the semaphore count is > 1,
    // proofs run on server threads pool could starve it.
    let generated_proof = spawn_blocking(move || prover.prove(&prover_input))
        .await
        .map_err(|join_error| {
            AppError::ProverError(format!("proving task failed: {join_error}"))
        })??;

    info!(elapsed = ?proving_started.elapsed(), "Proof generated");
    Ok(generated_proof)
}

/// The circuit's chunk amount, nullifier and root equal the withdrawal's.
fn check_public_inputs(
    public_inputs_be: &[[u8; 32]; PUBLIC_INPUTS_LEN],
    withdrawal_witness: &WithdrawalWitness,
    merkle_path: &MerklePath<TREE_DEPTH>,
) -> RequestResult<()> {
    let [_, chunk_amount_be, _, nullifier_be, root_be] = public_inputs_be;

    let expected_chunk_amount_be = fr_to_be(&Fr::from(withdrawal_witness.chunk_amount()));
    let chunk_amount_matches = *chunk_amount_be == expected_chunk_amount_be;
    let nullifier_matches = *nullifier_be == withdrawal_witness.nullifier_be;
    let root_matches = *root_be == fr_to_be(&merkle_path.root);

    if chunk_amount_matches && nullifier_matches && root_matches {
        return Ok(());
    }

    Err(AppError::ProverError(format!(
        "the proof's public inputs differ from the withdrawal: chunk amount matches \
         {chunk_amount_matches}, nullifier matches {nullifier_matches}, root matches {root_matches}"
    )))
}

/// Uploads the proof, sends the withdraw tx and records the withdrawal once the tx is confirmed.
async fn upload_proof_and_withdraw(
    state: &AppState,
    request: &WithdrawRequest,
    withdrawal_witness: &WithdrawalWitness,
    proof_hash: [u8; 32],
    generated_proof: GeneratedProof,
) -> RequestResult<WithdrawalTxs> {
    let upload_txs = upload_proof(
        state,
        &request.fee_payer,
        proof_hash,
        &generated_proof.proof,
    )
    .await?;

    let withdraw_tx = send_withdraw(
        state,
        request,
        withdrawal_witness.recipient(),
        proof_hash,
        generated_proof.public_inputs_be,
    )
    .await?;

    record_withdrawal(
        state,
        &request.deposit_id,
        request.step,
        Some(withdraw_tx),
        &withdrawal_witness.nullifier_be,
    )
    .await
    .inspect_err(|update_error| {
        error!(deposit_id = %request.deposit_id, step = request.step, %withdraw_tx, %update_error, "Recording the confirmed withdrawal failed");
    })?;

    Ok(WithdrawalTxs {
        upload_txs,
        withdraw_tx,
    })
}

/// The withdraw tx was rejected because the proof's root left the on-chain root history.
fn is_unknown_root_rejection(withdrawal_error: &AppError) -> bool {
    let AppError::TxRejected(TransactionError::InstructionError(
        _,
        InstructionError::Custom(error_code),
    )) = withdrawal_error
    else {
        return false;
    };

    // The generated error enum's discriminants are the IDL error codes.
    *error_code == ZkShieldedPoolSolanaError::UnknownRoot as u32
}

/// Part 0 resets the proof account, so it goes first.
async fn upload_proof(
    state: &AppState,
    fee_payer: &Keypair,
    proof_hash: [u8; 32],
    proof: &[u8],
) -> RequestResult<[Signature; 2]> {
    let proof_final_len = u16::try_from(proof.len()).map_err(|_| {
        AppError::ProverError(format!(
            "a {} byte proof does not fit proof_final_len",
            proof.len()
        ))
    })?;

    let (proof_part0, proof_part1) = proof.split_at(PROOF_UPLOAD_PART0_LEN);

    let upload_part0_ix = state.pool.upload_proof(
        fee_payer.pubkey(),
        proof_hash,
        0,
        proof_final_len,
        proof_part0.to_vec(),
    );

    let upload_part0_tx = send_v0(&state.rpc, fee_payer, &[upload_part0_ix]).await?;

    let upload_part1_ix = state.pool.upload_proof(
        fee_payer.pubkey(),
        proof_hash,
        1,
        proof_final_len,
        proof_part1.to_vec(),
    );

    let upload_part1_tx = send_v0(&state.rpc, fee_payer, &[upload_part1_ix]).await?;

    Ok([upload_part0_tx, upload_part1_tx])
}

/// Signs before sending, so a failed send still logs the tx signature.
async fn send_withdraw(
    state: &AppState,
    request: &WithdrawRequest,
    recipient: Pubkey,
    proof_hash: [u8; 32],
    public_inputs_be: [[u8; 32]; PUBLIC_INPUTS_LEN],
) -> RequestResult<Signature> {
    let [step, chunk_amount, dest_address, nullifier, root] = public_inputs_be;

    let withdraw_ix = state.pool.withdraw(
        request.fee_payer.pubkey(),
        recipient,
        proof_hash,
        PublicInputs {
            step,
            chunk_amount,
            dest_address,
            nullifier,
            root,
        },
    );

    let [compute_unit_limit_ix, heap_frame_ix] =
        compute_budget_ixs(WITHDRAW_CU_LIMIT, WITHDRAW_HEAP_BYTES);

    let blockhash = state
        .rpc
        .get_latest_blockhash()
        .await
        .map_err(create_rpc_error)?;

    let withdraw_tx = compile_v0(
        &request.fee_payer,
        &[compute_unit_limit_ix, heap_frame_ix, withdraw_ix],
        blockhash,
    )?;

    let withdraw_tx_signature = withdraw_tx.signatures[0];

    state
        .rpc
        .send_and_confirm_transaction(&withdraw_tx)
        .await
        .map_err(|send_error| {
            let send_error = create_rpc_error(send_error);
            error!(deposit_id = %request.deposit_id, step = request.step, %withdraw_tx_signature, %send_error, "Withdraw tx send failed");
            send_error
        })
}

async fn record_withdrawal(
    state: &AppState,
    deposit_id: &str,
    step: u8,
    withdraw_tx: Option<Signature>,
    nullifier_be: &[u8; 32],
) -> RequestResult<()> {
    let withdrawal_record = WithdrawalRecord {
        tx: withdraw_tx.map(|withdraw_tx| withdraw_tx.to_string()),
        nullifier_hex: hex::encode(nullifier_be),
        at: Utc::now(),
    };

    state
        .zk_deposit_data_writer
        .update(deposit_id, |zk_deposit_data| {
            zk_deposit_data.withdrawals[usize::from(step)] = Some(withdrawal_record);
        })
        .await?;

    Ok(())
}

/// Best effort: returns the rent of the proof account to the fee payer.
async fn close_proof(
    state: &AppState,
    fee_payer: &Keypair,
    proof_hash: [u8; 32],
) -> Option<Signature> {
    let close_proof_ix = state.pool.close_proof(fee_payer.pubkey(), proof_hash);

    send_v0(&state.rpc, fee_payer, &[close_proof_ix])
        .await
        .inspect_err(|close_proof_error| {
            let proof_pda = derive_proof_pda(&fee_payer.pubkey(), &proof_hash);
            warn!(%proof_pda, %close_proof_error, "Closing the proof account failed, its rent stays there");
        })
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only an `UnknownRoot` rejection makes the withdrawal run again.
    #[test]
    fn only_an_unknown_root_rejection_is_retried() {
        let unknown_root_error = AppError::TxRejected(TransactionError::InstructionError(
            2,
            InstructionError::Custom(ZkShieldedPoolSolanaError::UnknownRoot as u32),
        ));
        assert!(is_unknown_root_rejection(&unknown_root_error));

        let invalid_proof_error = AppError::TxRejected(TransactionError::InstructionError(
            2,
            InstructionError::Custom(ZkShieldedPoolSolanaError::InvalidProof as u32),
        ));
        assert!(!is_unknown_root_rejection(&invalid_proof_error));

        let unknown_outcome_error = AppError::RpcError("unable to confirm transaction".into());
        assert!(!is_unknown_root_rejection(&unknown_outcome_error));
    }
}
