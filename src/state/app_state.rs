use super::{
    config::Config,
    user_wallets::{UserWallets, WalletKind},
};
use crate::{
    app_error::RawResult,
    solana::{constants::RPC_COMMITMENT, instructions::ZkPoolProgram},
    zk::zk_deposit_data::ZkDepositDataWriter,
};
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    /// Sends and confirms transactions, reads accounts.
    pub rpc: Arc<RpcClient>,
    /// Builds instructions only.
    pub pool: Arc<ZkPoolProgram>,
    pub deposit_wallets: Arc<UserWallets>,
    pub withdraw_wallets: Arc<UserWallets>,
    pub notes: Arc<ZkDepositDataWriter>,
}

impl AppState {
    pub fn new(config: Config) -> RawResult<Self> {
        let rpc = RpcClient::new_with_commitment(config.solana_rpc_url.clone(), RPC_COMMITMENT);
        let pool = ZkPoolProgram::new(config.cluster())?;
        let deposit_wallets = UserWallets::new(WalletKind::Deposit, &config.deposit_wallet_keys);
        let withdraw_wallets = UserWallets::new(WalletKind::Withdraw, &config.withdraw_wallet_keys);
        let notes = ZkDepositDataWriter::new(&config.zk_deposit_data_dir)?;

        Ok(Self {
            config: Arc::new(config),
            rpc: Arc::new(rpc),
            pool: Arc::new(pool),
            deposit_wallets: Arc::new(deposit_wallets),
            withdraw_wallets: Arc::new(withdraw_wallets),
            notes: Arc::new(notes),
        })
    }
}
