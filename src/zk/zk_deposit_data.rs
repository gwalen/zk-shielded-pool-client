use std::{
    fmt,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
};

use anyhow::format_err;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use shielded_pool_circuit::circuit::utils::fr_to_le_bytes;
use tokio::{fs, io::AsyncWriteExt, sync::Mutex};

use super::field::{CHUNKS, Fr, fr_from_le};
use crate::app_error::{AppError, RawResult, RequestResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZkDepositStatus {
    /// Written before the deposit tx is sent.
    Pending,
    Confirmed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithdrawalRecord {
    /// Withdraw tx signature; `None` when the nullifier was spent on chain before a recorded withdraw.
    pub tx: Option<String>,
    /// BE hex, the same bytes as the nullifier PDA seed.
    pub nullifier_hex: String,
    pub at: DateTime<Utc>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZkDepositData {
    /// hex(deposit_commitment LE). Public, stable, also the file name.
    pub id: String,
    /// hex LE. Secret: never log it.
    pub secret_s: String,
    /// base58 deposit wallet that signed the deposit. A record only, withdraw never uses it.
    pub deposit_wallet: String,
    pub total_amount: u64,
    pub chunks: [u64; CHUNKS],
    /// base58 recipients, one per chunk.
    pub destinations: [String; CHUNKS],
    /// hex LE
    pub user_commitment_hash: String,
    /// hex LE
    pub deposit_commitment_hash: String,
    pub status: ZkDepositStatus,
    /// Signature of the signed deposit tx. Stored before sending, so the tx may not have landed.
    pub deposit_tx: String,
    /// Why the deposit failed or could not be verified. Never contains the secret.
    #[serde(default)]
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub withdrawals: [Option<WithdrawalRecord>; CHUNKS],
}

impl ZkDepositData {
    /// Id for a deposit commitment: hex of its LE bytes.
    pub fn create_id(deposit_commitment: Fr) -> String {
        hex::encode(fr_to_le_bytes(deposit_commitment))
    }

    /// `s` as a field element.
    pub fn secret(&self) -> RequestResult<Fr> {
        parse_fr_hex(&self.secret_s).ok_or_else(|| {
            AppError::Internal(format_err!("zk deposit data {}: invalid secret_s", self.id))
        })
    }

    /// Funded chunks still to withdraw on a confirmed deposit.
    pub fn open_chunk_count(&self) -> u64 {
        if self.status == ZkDepositStatus::Confirmed {
            return self.count_chunks_awaiting_withdrawal();
        }
        0
    }

    /// Funded chunks on a pending deposit.
    pub fn pending_chunk_count(&self) -> u64 {
        if self.status == ZkDepositStatus::Pending {
            return self.count_chunks_awaiting_withdrawal();
        }
        0
    }

    /// Funded chunks whose withdrawal slot is empty.
    fn count_chunks_awaiting_withdrawal(&self) -> u64 {
        let mut awaiting_withdrawal_chunk_count = 0u64;

        for (chunk_lamports, withdrawal_record) in self.chunks.iter().zip(self.withdrawals.iter()) {
            if *chunk_lamports != 0 && withdrawal_record.is_none() {
                awaiting_withdrawal_chunk_count += 1;
            }
        }

        awaiting_withdrawal_chunk_count
    }

    /// Chunks that have a withdrawal record.
    pub fn withdrawn_chunk_count(&self) -> u64 {
        let mut withdrawn_chunk_count = 0u64;

        for withdrawal_record in &self.withdrawals {
            if withdrawal_record.is_some() {
                withdrawn_chunk_count += 1;
            }
        }

        withdrawn_chunk_count
    }

    /// This confirmed deposit still has a funded chunk to withdraw.
    pub fn is_open(&self) -> bool {
        self.open_chunk_count() > 0
    }

    /// Confirmed, with a withdrawal stored for every funded chunk.
    pub fn is_fully_withdrawn(&self) -> bool {
        if self.status != ZkDepositStatus::Confirmed {
            return false;
        }
        self.open_chunk_count() == 0 && self.withdrawn_chunk_count() > 0
    }
}

/// Same fields as the derive, but `secret_s` is redacted.
impl fmt::Debug for ZkDepositData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ZkDepositData")
            .field("id", &self.id)
            .field("secret_s", &"<redacted>")
            .field("deposit_wallet", &self.deposit_wallet)
            .field("total_amount", &self.total_amount)
            .field("chunks", &self.chunks)
            .field("destinations", &self.destinations)
            .field("user_commitment_hash", &self.user_commitment_hash)
            .field("deposit_commitment_hash", &self.deposit_commitment_hash)
            .field("status", &self.status)
            .field("deposit_tx", &self.deposit_tx)
            .field("error", &self.error)
            .field("created_at", &self.created_at)
            .field("withdrawals", &self.withdrawals)
            .finish()
    }
}

/// hex LE -> Fr. `None` if not 32 bytes of hex or not a canonical field element.
pub fn parse_fr_hex(value: &str) -> Option<Fr> {
    let bytes: [u8; 32] = hex::decode(value).ok()?.try_into().ok()?;
    fr_from_le(bytes)
}

/// File store: `<dir>/<id>.json`. Writes are atomic (temp file + rename), so a
/// reader never sees a half-written file. `write_lock` serialises `update` (read-change-write).
pub struct ZkDepositDataWriter {
    dir: PathBuf,
    write_lock: Mutex<()>,
}

impl ZkDepositDataWriter {
    /// Creates `dir` if it is missing.
    pub fn new(dir: impl Into<PathBuf>) -> RawResult<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            write_lock: Mutex::new(()),
        })
    }

    /// Store the data of a new deposit. `AppError::Conflict` (HTTP 409) if a file with this id
    /// already exists: the same deposit commitment was stored before, and it is never overwritten.
    pub async fn create(&self, zk_deposit_data: &ZkDepositData) -> RequestResult<()> {
        let path = self.path(&zk_deposit_data.id)?;
        // No lock: the id is 32 random bytes, so two creates never race on the same file.
        if fs::try_exists(&path).await.map_err(create_io_error)? {
            return Err(AppError::Conflict(format!(
                "zk deposit data {} already exists",
                zk_deposit_data.id
            )));
        }

        self.write_atomic(&path, zk_deposit_data).await
    }

    /// `AppError::NotFound` (HTTP 404) if no deposit with this id was stored, e.g. a withdraw
    /// request with an unknown `deposit_id`.
    pub async fn get(&self, id: &str) -> RequestResult<ZkDepositData> {
        read(&self.path(id)?, id).await
    }

    /// Every `{deposit id}.json` file in the store.
    pub async fn fetch_all(&self) -> RequestResult<Vec<ZkDepositData>> {
        let mut read_dir = fs::read_dir(&self.dir).await.map_err(create_io_error)?;
        let mut zk_deposit_data_list = Vec::new();

        while let Some(dir_entry) = read_dir.next_entry().await.map_err(create_io_error)? {
            let path = dir_entry.path();

            let Some(id) = parse_zk_deposit_file_id(&path) else {
                continue;
            };

            zk_deposit_data_list.push(read(&path, &id).await?);
        }

        Ok(zk_deposit_data_list)
    }

    /// Read, change and write back one zk deposit data file under the store lock. Returns the new value.
    pub async fn update(
        &self,
        id: &str,
        change: impl FnOnce(&mut ZkDepositData),
    ) -> RequestResult<ZkDepositData> {
        let path = self.path(id)?;

        let _guard = self.write_lock.lock().await;

        let mut zk_deposit_data = read(&path, id).await?;
        change(&mut zk_deposit_data);
        if zk_deposit_data.id != id {
            return Err(AppError::Internal(format_err!(
                "zk deposit data {id}: update changed the id"
            )));
        }
        self.write_atomic(&path, &zk_deposit_data).await?;

        drop(_guard); // reals the mutex lock

        Ok(zk_deposit_data)
    }

    /// Ids are lowercase hex only, as `create_id` writes them, so a request can never point
    /// outside `dir` and each deposit has exactly one id.
    fn path(&self, id: &str) -> RequestResult<PathBuf> {
        if !is_zk_deposit_id(id) {
            return Err(AppError::BadRequest(
                "deposit id must be 64 lowercase hex characters".into(),
            ));
        }
        Ok(self.dir.join(format!("{id}.json")))
    }

    // write file to tmp file first, if there was no error rename the file to the name, this avoids half-written files
    // which in case of a power loss or other unexpected error, the file would be corrupted and the secret "s" would be lost
    async fn write_atomic(
        &self,
        path: &Path,
        zk_deposit_data: &ZkDepositData,
    ) -> RequestResult<()> {
        let json = serde_json::to_vec_pretty(zk_deposit_data) // give the file a human-readable format
            .map_err(|e| {
                AppError::Internal(format_err!(
                    "serialize zk deposit data {}: {e}",
                    zk_deposit_data.id
                ))
            })?;
        let tmp = path.with_extension("json.tmp");

        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);

        let mut file = options.open(&tmp).await.map_err(create_io_error)?;
        file.write_all(&json).await.map_err(create_io_error)?;
        file.sync_all().await.map_err(create_io_error)?; // (fsync) waits until the data is really written to the disk as this is a critical operation to save secret "s"

        drop(file); // drop the file handle

        fs::rename(&tmp, path).await.map_err(create_io_error)
    }
}

fn is_zk_deposit_id(id: &str) -> bool {
    let is_lowercase_hex = id
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));

    id.len() == 64 && is_lowercase_hex
}

/// Deposit id of a `{id}.json` file.
fn parse_zk_deposit_file_id(path: &Path) -> Option<String> {
    if path.extension()? != "json" {
        return None;
    }

    let id = path.file_stem()?.to_str()?;
    if is_zk_deposit_id(id) {
        Some(id.to_string())
    } else {
        None
    }
}

async fn read(path: &Path, id: &str) -> RequestResult<ZkDepositData> {
    let bytes = match fs::read(path).await {
        Ok(bytes) => bytes,
        Err(e) => {
            if e.kind() == ErrorKind::NotFound {
                return Err(AppError::NotFound(format!(
                    "zk deposit data not found: {id}"
                )));
            } else {
                return Err(create_io_error(e));
            }
        }
    };
    // serde_json errors name the position and expected type, not the values, so no secret leaks.
    serde_json::from_slice(&bytes)
        .map_err(|e| AppError::Internal(format_err!("zk deposit data {id} is corrupt: {e}")))
}

fn create_io_error(e: io::Error) -> AppError {
    AppError::Internal(format_err!("zk deposit data internal error: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_sample_deposit(id_byte: u8) -> ZkDepositData {
        ZkDepositData {
            id: hex::encode([id_byte; 32]),
            secret_s: hex::encode(fr_to_le_bytes(Fr::from(1_234_567_890u64))),
            deposit_wallet: "Wallet1111111111111111111111111111111111111".into(),
            total_amount: 9,
            chunks: [2, 3, 4],
            destinations: ["a".into(), "b".into(), "c".into()],
            user_commitment_hash: hex::encode([1; 32]),
            deposit_commitment_hash: hex::encode([id_byte; 32]),
            status: ZkDepositStatus::Pending,
            deposit_tx: "sig".into(),
            error: None,
            created_at: Utc::now(),
            withdrawals: [None, None, None],
        }
    }

    /// create -> get -> update -> get, plus the two errors the API turns into HTTP statuses.
    #[tokio::test]
    async fn create_get_update() {
        let dir = tempfile::tempdir().unwrap();
        let store = ZkDepositDataWriter::new(dir.path()).unwrap();
        let deposit = create_sample_deposit(0xAB);

        store.create(&deposit).await.unwrap();
        assert_eq!(store.get(&deposit.id).await.unwrap(), deposit);
        // Storing the same deposit twice is refused (409 Conflict), never overwritten.
        assert!(matches!(
            store.create(&deposit).await,
            Err(AppError::Conflict(_))
        ));

        let updated = store
            .update(&deposit.id, |d| {
                d.status = ZkDepositStatus::Confirmed;
            })
            .await
            .unwrap();
        assert_eq!(updated.status, ZkDepositStatus::Confirmed);
        assert_eq!(store.get(&deposit.id).await.unwrap(), updated);

        let missing = hex::encode([0xCD; 32]);
        // An id that was never stored (404 Not Found), e.g. a withdraw for an unknown deposit.
        assert!(matches!(
            store.get(&missing).await,
            Err(AppError::NotFound(_))
        ));
    }

    /// An uppercase id opens the same file on a case-insensitive disk, but it does not match the
    /// stored id, so `update` would fail after a confirmed withdraw tx.
    #[tokio::test]
    async fn get_rejects_uppercase_id() {
        let dir = tempfile::tempdir().unwrap();
        let store = ZkDepositDataWriter::new(dir.path()).unwrap();
        let deposit = create_sample_deposit(0xAB);
        store.create(&deposit).await.unwrap();

        let uppercase_id = deposit.id.to_uppercase();
        assert!(matches!(
            store.get(&uppercase_id).await,
            Err(AppError::BadRequest(_))
        ));
    }

    /// Open and withdrawn counts require a confirmed deposit.
    #[test]
    fn open_and_withdrawn_follow_the_withdrawal_records() {
        let mut zk_deposit_data = create_sample_deposit(1);
        assert_eq!(zk_deposit_data.status, ZkDepositStatus::Pending);
        assert_eq!(zk_deposit_data.open_chunk_count(), 0);
        assert_eq!(zk_deposit_data.pending_chunk_count(), 3);
        assert!(!zk_deposit_data.is_open());
        assert!(!zk_deposit_data.is_fully_withdrawn());

        zk_deposit_data.status = ZkDepositStatus::Failed;
        assert_eq!(zk_deposit_data.open_chunk_count(), 0);
        assert_eq!(zk_deposit_data.pending_chunk_count(), 0);
        assert!(!zk_deposit_data.is_open());
        assert!(!zk_deposit_data.is_fully_withdrawn());

        zk_deposit_data.status = ZkDepositStatus::Confirmed;
        assert_eq!(zk_deposit_data.open_chunk_count(), 3);
        assert_eq!(zk_deposit_data.pending_chunk_count(), 0);
        assert_eq!(zk_deposit_data.withdrawn_chunk_count(), 0);
        assert!(zk_deposit_data.is_open());
        assert!(!zk_deposit_data.is_fully_withdrawn());

        zk_deposit_data.chunks = [2, 0, 4];
        assert_eq!(zk_deposit_data.open_chunk_count(), 2);
        assert_eq!(zk_deposit_data.withdrawn_chunk_count(), 0);

        zk_deposit_data.withdrawals[0] = Some(WithdrawalRecord {
            tx: Some("sig".into()),
            nullifier_hex: hex::encode([9u8; 32]),
            at: Utc::now(),
        });
        assert_eq!(zk_deposit_data.open_chunk_count(), 1);
        assert_eq!(zk_deposit_data.withdrawn_chunk_count(), 1);
        assert!(zk_deposit_data.is_open());
        assert!(!zk_deposit_data.is_fully_withdrawn());

        // Withdrawal recorded after the nullifier was already spent on chain.
        zk_deposit_data.withdrawals[2] = Some(WithdrawalRecord {
            tx: None,
            nullifier_hex: hex::encode([8u8; 32]),
            at: Utc::now(),
        });
        assert_eq!(zk_deposit_data.open_chunk_count(), 0);
        assert_eq!(zk_deposit_data.withdrawn_chunk_count(), 2);
        assert!(!zk_deposit_data.is_open());
        assert!(zk_deposit_data.is_fully_withdrawn());

        zk_deposit_data.status = ZkDepositStatus::Failed;
        assert_eq!(zk_deposit_data.open_chunk_count(), 0);
        assert!(!zk_deposit_data.is_fully_withdrawn());

        zk_deposit_data.status = ZkDepositStatus::Confirmed;
        zk_deposit_data.chunks = [0, 0, 0];
        zk_deposit_data.withdrawals = [None, None, None];
        assert_eq!(zk_deposit_data.open_chunk_count(), 0);
        assert_eq!(zk_deposit_data.withdrawn_chunk_count(), 0);
        assert!(!zk_deposit_data.is_open());
        assert!(!zk_deposit_data.is_fully_withdrawn());
    }

    #[tokio::test]
    async fn fetch_all_reads_deposit_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = ZkDepositDataWriter::new(dir.path()).unwrap();
        let first = create_sample_deposit(0x11);
        let second = create_sample_deposit(0x22);
        store.create(&first).await.unwrap();
        store.create(&second).await.unwrap();

        let temp_path = dir.path().join(format!("{}.json.tmp", first.id));
        std::fs::write(&temp_path, b"{}").unwrap();
        std::fs::write(dir.path().join("notes.json"), b"{}").unwrap();

        let mut ids: Vec<_> = store
            .fetch_all()
            .await
            .unwrap()
            .into_iter()
            .map(|zk_deposit_data| zk_deposit_data.id)
            .collect();
        ids.sort();

        let mut expected = vec![first.id, second.id];
        expected.sort();
        assert_eq!(ids, expected);
    }

    #[tokio::test]
    async fn fetch_all_rejects_a_corrupt_deposit_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = ZkDepositDataWriter::new(dir.path()).unwrap();
        let corrupt_id = hex::encode([0x33; 32]);
        let corrupt_path = dir.path().join(format!("{corrupt_id}.json"));
        std::fs::write(&corrupt_path, b"not-json").unwrap();

        let err = store.fetch_all().await.unwrap_err();
        assert!(err.to_string().contains("corrupt"), "{err}");
    }

    #[test]
    fn debug_redacts_the_secret() {
        let deposit = create_sample_deposit(2);
        let rendered = format!("{deposit:?}");
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains(&deposit.secret_s));
    }
}
