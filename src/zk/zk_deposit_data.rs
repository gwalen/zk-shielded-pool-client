use std::{
    fmt,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
};

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
    pub tx: String,
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
    pub deposit_tx: Option<String>,
    /// Why the deposit failed (`status == Failed`). Never contains the secret.
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
            AppError::Internal(anyhow::anyhow!("zk deposit data {}: invalid secret_s", self.id))
        })
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
            return Err(AppError::Internal(anyhow::anyhow!(
                "zk deposit data {id}: update changed the id"
            )));
        }
        self.write_atomic(&path, &zk_deposit_data).await?;

        drop(_guard); // reals the mutex lock

        Ok(zk_deposit_data)
    }

    /// Ids are hex only, so a request can never point outside `dir`.
    fn path(&self, id: &str) -> RequestResult<PathBuf> {
        let valid = id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit());
        if !valid {
            return Err(AppError::BadRequest(
                "deposit id must be 64 hex characters".into(),
            ));
        }
        Ok(self.dir.join(format!("{id}.json")))
    }

    // write file to tmp file first, if there was no error rename the file to the name, this avoids half-written files
    // which in case of a power loss or other unexpected error, the file would be corrupted and the secret "s" would be lost
    async fn write_atomic(&self, path: &Path, zk_deposit_data: &ZkDepositData) -> RequestResult<()> {
        let json = serde_json::to_vec_pretty(zk_deposit_data) // give the file a human-readable format
            .map_err(|e| AppError::Internal(anyhow::anyhow!("serialize zk deposit data {}: {e}", zk_deposit_data.id)))?;
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

async fn read(path: &Path, id: &str) -> RequestResult<ZkDepositData> {
    let bytes = match fs::read(path).await {
        Ok(bytes) => bytes,
        Err(e) => {
            if e.kind() == ErrorKind::NotFound {
                return Err(AppError::NotFound(format!("zk deposit data not found: {id}")));
            } else {
                return Err(create_io_error(e));
            }
        }
    };
    // serde_json errors name the position and expected type, not the values, so no secret leaks.
    serde_json::from_slice(&bytes)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("zk deposit data {id} is corrupt: {e}")))
}

fn create_io_error(e: io::Error) -> AppError {
    AppError::Internal(anyhow::anyhow!("zk deposit data internal error: {e}"))
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
            deposit_tx: None,
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
        assert!(matches!(store.create(&deposit).await, Err(AppError::Conflict(_))));

        let updated = store
            .update(&deposit.id, |d| {
                d.status = ZkDepositStatus::Confirmed;
                d.deposit_tx = Some("sig".into());
            })
            .await
            .unwrap();
        assert_eq!(updated.status, ZkDepositStatus::Confirmed);
        assert_eq!(store.get(&deposit.id).await.unwrap(), updated);

        let missing = hex::encode([0xCD; 32]);
        // An id that was never stored (404 Not Found), e.g. a withdraw for an unknown deposit.
        assert!(matches!(store.get(&missing).await, Err(AppError::NotFound(_))));
    }

    #[test]
    fn debug_redacts_the_secret() {
        let deposit = create_sample_deposit(2);
        let rendered = format!("{deposit:?}");
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains(&deposit.secret_s));
    }
}
