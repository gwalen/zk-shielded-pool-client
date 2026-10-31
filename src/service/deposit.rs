//! Deposit use case: commit, store the zk deposit data, send the deposit tx, check the event.

use std::sync::Arc;

use chrono::Utc;
use ff::Field;
use rand::rngs::OsRng;
use shielded_pool_circuit::circuit::{
    full_circuit::{deposit_commitment_hash, user_commitment_hash},
    utils::{convert_pubkey_32bytes_to_fr, fr_to_le_bytes},
};
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_signer::Signer;
use tracing::error;

use crate::{
    app_error::{AppError, RequestResult},
    solana::{
        accounts::ProgramConfigView,
        accounts_helpers::fetch_program_config,
        events::{DepositDone, parse_deposit_done_events},
        tx_builder::{compile_v0, create_rpc_error, transaction_logs},
    },
    state::AppState,
    zk::{
        field::{CHUNKS, Fr},
        zk_deposit_data::{ZkDepositData, ZkDepositStatus},
    },
};

/// A deposit request whose wallet, destinations and total are already checked.
pub struct DepositRequest {
    /// Configured deposit wallet: signer and fee payer of the deposit tx.
    pub depositor: Arc<Keypair>,
    pub chunks: [u64; CHUNKS],
    pub destinations: [Pubkey; CHUNKS],
    /// Sum of the chunks.
    pub total_amount: u64,
}

/// A confirmed deposit whose event matches the locally computed commitment.
pub struct DepositReceipt {
    pub zk_deposit_data: ZkDepositData,
    pub tx_signature: Signature,
    /// Tree root after the insert, LE.
    pub new_root_le: [u8; 32],
}

/// Secret `s` and the two commitments of one deposit.
struct DepositCommitments {
    secret_s: Fr,
    user_commitment_hash: Fr,
    deposit_commitment_hash: Fr,
}

impl DepositCommitments {
    fn compute(secret_s: Fr, request: &DepositRequest) -> Self {
        let chunks = request.chunks.map(Fr::from);

        let destinations = request
            .destinations
            .map(|destination| convert_pubkey_32bytes_to_fr(destination.to_bytes()));

        let user_commitment_hash = user_commitment_hash(secret_s, &chunks, &destinations);

        let deposit_commitment_hash =
            deposit_commitment_hash(user_commitment_hash, Fr::from(request.total_amount));

        Self {
            secret_s,
            user_commitment_hash,
            deposit_commitment_hash,
        }
    }

    fn create_pending_zk_deposit_data(
        &self,
        request: &DepositRequest,
        tx_signature: &Signature,
    ) -> ZkDepositData {
        ZkDepositData {
            id: ZkDepositData::create_id(self.deposit_commitment_hash),
            secret_s: hex::encode(fr_to_le_bytes(self.secret_s)),
            deposit_wallet: request.depositor.pubkey().to_string(),
            total_amount: request.total_amount,
            chunks: request.chunks,
            destinations: request
                .destinations
                .map(|destination| destination.to_string()),
            user_commitment_hash: hex::encode(fr_to_le_bytes(self.user_commitment_hash)),
            deposit_commitment_hash: hex::encode(fr_to_le_bytes(self.deposit_commitment_hash)),
            status: ZkDepositStatus::Pending,
            deposit_tx: Some(tx_signature.to_string()),
            error: None,
            created_at: Utc::now(),
            withdrawals: [None, None, None],
        }
    }
}

/// Stores the zk deposit data and the tx signature before sending, so both survive a crash after the tx lands.
/// A rejected tx marks the zk deposit data `Failed`; an unknown send outcome or a failed event check keeps it `Pending`.
pub async fn deposit(state: &AppState, request: DepositRequest) -> RequestResult<DepositReceipt> {
    let pool_config = fetch_program_config(&state.rpc)
        .await?
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("the pool is not initialized")))?;

    check_pool_accepts_deposit(&pool_config, request.total_amount)?;

    let commitments = DepositCommitments::compute(Fr::random(OsRng), &request);

    let ix = state.pool.deposit(
        request.depositor.pubkey(),
        fr_to_le_bytes(commitments.user_commitment_hash),
        request.total_amount,
    );
    let blockhash = state
        .rpc
        .get_latest_blockhash()
        .await
        .map_err(create_rpc_error)?;
    let deposit_tx = compile_v0(&request.depositor, &[ix], blockhash)?;
    let tx_signature = deposit_tx.signatures[0];

    let zk_deposit_data = commitments.create_pending_zk_deposit_data(&request, &tx_signature);
    state
        .zk_deposit_data_writer
        .create(&zk_deposit_data)
        .await?;

    let send_result = state
        .rpc
        .send_and_confirm_transaction(&deposit_tx)
        .await
        .map_err(create_rpc_error);

    if let Err(send_error) = send_result {
        error!(deposit_id = %zk_deposit_data.id, %tx_signature, %send_error, "Deposit tx send failed");
        state
            .zk_deposit_data_writer
            .update(&zk_deposit_data.id, |zk_deposit_data| {
                record_send_error(zk_deposit_data, &send_error)
            })
            .await?;
        return Err(send_error);
    }

    let new_root_result = transaction_logs(&state.rpc, &tx_signature)
        .await
        .and_then(|logs| parse_deposit_done_events(&logs))
        .and_then(|events| check_deposit_done(&events, &commitments, request.total_amount));

    let zk_deposit_data = state
        .zk_deposit_data_writer
        .update(&zk_deposit_data.id, |zk_deposit_data| {
            record_event_check(zk_deposit_data, &new_root_result)
        })
        .await
        .map_err(|update_error| {
            error!(deposit_id = %zk_deposit_data.id, %tx_signature, %update_error, "Deposit tx landed but the zk deposit data update failed");
            update_error
        })?;

    // extract new new root value
    let new_root = new_root_result.map_err(|error| {
        AppError::Internal(anyhow::anyhow!(
            "deposit {} tx {tx_signature} landed but is unverified, the zk deposit data stays pending: {error}",
            zk_deposit_data.id
        ))
    })?;

    Ok(DepositReceipt {
        zk_deposit_data,
        tx_signature,
        new_root_le: new_root,
    })
}

/// The program rejects a paused pool and a total below the minimum. Checked before any tx is sent.
fn check_pool_accepts_deposit(
    pool_config: &ProgramConfigView,
    total_amount: u64,
) -> RequestResult<()> {
    if pool_config.paused {
        return Err(AppError::BadRequest(
            "the pool is paused, deposits are disabled".into(),
        ));
    }

    if total_amount < pool_config.min_deposit_lamports {
        return Err(AppError::BadRequest(format!(
            "total {total_amount} lamports is below the pool minimum of {} lamports",
            pool_config.min_deposit_lamports
        )));
    }

    Ok(())
}

/// Exactly one `DepositDone` with our commitment and total. Returns the new root (LE).
fn check_deposit_done(
    events: &[DepositDone],
    commitments: &DepositCommitments,
    total_amount: u64,
) -> RequestResult<[u8; 32]> {
    let [event] = events else {
        return Err(AppError::Internal(anyhow::anyhow!(
            "expected exactly one DepositDone event, found {}",
            events.len()
        )));
    };

    let expected = fr_to_le_bytes(commitments.deposit_commitment_hash);
    let actual = event.deposit_commitment_hash.to_bytes();

    if actual != expected || event.total_amount != total_amount {
        return Err(AppError::Internal(anyhow::anyhow!(
            "DepositDone differs from the local commitment (client and program hash differently): \
             program {} / {} lamports, local {} / {total_amount} lamports",
            hex::encode(actual),
            event.total_amount,
            hex::encode(expected),
        )));
    }

    Ok(event.new_root.to_bytes())
}

/// `Failed` only for a definite rejection. Any other send error leaves the outcome unknown, so the status stays `Pending`.
fn record_send_error(zk_deposit_data: &mut ZkDepositData, send_error: &AppError) {
    if matches!(send_error, AppError::TxRejected(_)) {
        zk_deposit_data.status = ZkDepositStatus::Failed;
    }
    zk_deposit_data.error = Some(send_error.to_string());
}

/// The tx landed: `Confirmed` only if the event check passed.
fn record_event_check(zk_deposit_data: &mut ZkDepositData, checked_root: &RequestResult<[u8; 32]>) {
    match checked_root {
        Ok(_) => zk_deposit_data.status = ZkDepositStatus::Confirmed,
        Err(error) => zk_deposit_data.error = Some(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shielded_pool_circuit::imt::imt_utils::{generate_zero_values_for_levels, poseidon_hash};

    use shielded_pool_circuit::transcript::fr_to_be;

    use crate::zk::field::TREE_DEPTH;

    const FIXTURE_PUBLIC_INPUTS: &[u8; 160] =
        include_bytes!("../../tests/fixtures/step0/public_inputs.bin");
    const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

    fn create_program_config_account_view(
        paused: bool,
        min_deposit_lamports: u64,
    ) -> ProgramConfigView {
        ProgramConfigView {
            owner: Pubkey::new_unique(),
            paused,
            min_deposit_lamports,
        }
    }

    fn create_fixture_request() -> DepositRequest {
        let destination: Pubkey = "dstH17g8RBGdUo3YeYhSFHDdFzHrWkAzNCKSveAchyD"
            .parse()
            .unwrap();
        DepositRequest {
            depositor: Arc::new(Keypair::new()),
            chunks: [2, 3, 4].map(|sol| sol * LAMPORTS_PER_SOL),
            destinations: [destination; CHUNKS],
            total_amount: 9 * LAMPORTS_PER_SOL,
        }
    }

    /// The fixture deposit (fixed `s`) as the only leaf of a depth-20 tree gives the fixture root.
    #[test]
    fn fixture_request_commits_to_the_fixture_root() {
        let request = create_fixture_request();

        let commitments = DepositCommitments::compute(Fr::from(1_234_567_890u64), &request);
        let root = generate_zero_values_for_levels(TREE_DEPTH)
            .into_iter()
            .fold(commitments.deposit_commitment_hash, poseidon_hash);
        assert_eq!(fr_to_be(&root), FIXTURE_PUBLIC_INPUTS[128..160]);

        let tx_signature = Signature::default();
        let zk_deposit_data = commitments.create_pending_zk_deposit_data(&request, &tx_signature);
        assert_eq!(zk_deposit_data.id, zk_deposit_data.deposit_commitment_hash);
        assert_eq!(zk_deposit_data.status, ZkDepositStatus::Pending);
        assert_eq!(zk_deposit_data.deposit_tx, Some(tx_signature.to_string()));
    }

    /// Withdraw refuses anything not `Confirmed`, so only a definite rejection may end a deposit as `Failed`.
    #[test]
    fn only_a_rejected_tx_marks_the_deposit_failed() {
        let request = create_fixture_request();
        let commitments = DepositCommitments::compute(Fr::from(1u64), &request);
        let pending_zk_deposit_data =
            commitments.create_pending_zk_deposit_data(&request, &Signature::default());

        let mut rejected_zk_deposit_data = pending_zk_deposit_data.clone();
        let rejected_error = AppError::TxRejected("instruction 0 failed".into());
        record_send_error(&mut rejected_zk_deposit_data, &rejected_error);
        assert_eq!(rejected_zk_deposit_data.status, ZkDepositStatus::Failed);
        assert_eq!(
            rejected_zk_deposit_data.error,
            Some(rejected_error.to_string())
        );

        let mut unknown_outcome_zk_deposit_data = pending_zk_deposit_data;
        let unknown_outcome_error = AppError::RpcError("unable to confirm transaction".into());
        record_send_error(&mut unknown_outcome_zk_deposit_data, &unknown_outcome_error);
        assert_eq!(
            unknown_outcome_zk_deposit_data.status,
            ZkDepositStatus::Pending
        );
        assert_eq!(
            unknown_outcome_zk_deposit_data.error,
            Some(unknown_outcome_error.to_string())
        );
    }

    #[test]
    fn total_below_minimum_is_rejected() {
        let program_config_account_view = create_program_config_account_view(false, 1_000);
        let err = check_pool_accepts_deposit(&program_config_account_view, 999).unwrap_err();
        assert!(
            matches!(err, AppError::BadRequest(ref msg) if msg.contains("below the pool minimum")),
            "{err}"
        );
        assert!(check_pool_accepts_deposit(&program_config_account_view, 1_000).is_ok());
    }

    #[test]
    fn paused_pool_is_rejected() {
        let err = check_pool_accepts_deposit(&create_program_config_account_view(true, 1), 1_000)
            .unwrap_err();
        assert!(
            matches!(err, AppError::BadRequest(ref msg) if msg.contains("paused")),
            "{err}"
        );
    }
}
