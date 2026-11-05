use std::{collections::HashMap, sync::Arc};

use tokio::sync::{Mutex, OwnedMutexGuard};

/// One lock per deposit id, so two withdraw requests for the same deposit run one after the other.
#[derive(Default)]
pub struct DepositLocks {
    locks: std::sync::Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl DepositLocks {
    /// Waits for this deposit's lock, released when the guard drops.
    pub async fn lock(&self, deposit_id: &str) -> OwnedMutexGuard<()> {
        let deposit_lock = Arc::clone(
            self.locks
                .lock()
                .expect("no thread panics while holding the deposit locks map")
                .entry(deposit_id.to_owned())
                .or_default(),
        );

        deposit_lock.lock_owned().await
    }
}
