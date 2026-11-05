use super::{
    config::Config,
    deposit_locks::DepositLocks,
    user_wallets::{UserWallets, WalletKind},
};
use crate::{
    app_error::RawResult,
    solana::{constants::RPC_COMMITMENT, instructions::ZkPoolProgram},
    zk::{prover::ProvingContext, zk_deposit_data::ZkDepositDataWriter},
};
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use std::sync::Arc;
use tokio::sync::Semaphore;

/// One proof at a time: halo2 already uses every core through rayon.
const PROVER_PERMITS: usize = 1;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    /// Sends and confirms transactions, reads accounts.
    pub rpc: Arc<RpcClient>,
    /// Builds instructions only.
    pub pool: Arc<ZkPoolProgram>,
    pub deposit_wallets: Arc<UserWallets>,
    pub withdraw_wallets: Arc<UserWallets>,
    pub zk_deposit_data_writer: Arc<ZkDepositDataWriter>,
    pub prover: Arc<ProvingContext>,
    pub prover_permits: Arc<Semaphore>,
    pub withdraw_locks: Arc<DepositLocks>,
}

impl AppState {
    /// `main` loads the prover off the async runtime: keygen takes seconds. It is an `Arc` so
    /// test servers can share one prover.
    pub fn new(config: Config, prover: Arc<ProvingContext>) -> RawResult<Self> {
        let rpc = RpcClient::new_with_commitment(config.solana_rpc_url.clone(), RPC_COMMITMENT);
        let pool = ZkPoolProgram::new(config.cluster())?;
        let deposit_wallets = UserWallets::new(WalletKind::Deposit, &config.deposit_wallet_keys);
        let withdraw_wallets = UserWallets::new(WalletKind::Withdraw, &config.withdraw_wallet_keys);
        let zk_deposit_data_writer = ZkDepositDataWriter::new(&config.zk_deposit_data_dir)?;

        Ok(Self {
            config: Arc::new(config),
            rpc: Arc::new(rpc),
            pool: Arc::new(pool),
            deposit_wallets: Arc::new(deposit_wallets),
            withdraw_wallets: Arc::new(withdraw_wallets),
            zk_deposit_data_writer: Arc::new(zk_deposit_data_writer),
            prover,
            prover_permits: Arc::new(Semaphore::new(PROVER_PERMITS)),
            withdraw_locks: Arc::new(DepositLocks::default()),
        })
    }
}
