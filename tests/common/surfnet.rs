//! Integration harness against a Surfpool surfnet (the Rust version of r3's `tests/utils/helpers.ts`).
//!
//! Tests using it are `#[ignore = "needs surfpool: run `just it`"]`. The surfnet keeps state
//! between test binaries, so tests compare before/after values instead of assuming an empty tree.
//!
//! Each `#[tokio::test]` has its own runtime, so RPC clients are created per test (a reqwest
//! client must not outlive the runtime it was built in). The pool owner is a fixed test key,
//! so every test binary can sign owner-only instructions (pause / unpause).

use std::{future::IntoFuture, sync::Arc, time::Duration};

use axum::http::StatusCode;
use serde_json::{Value, json};
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_rpc_client_api::request::RpcRequest;
use solana_signer::Signer;
use tempfile::TempDir;
use tokio::net::TcpListener;
use zk_shielded_pool_client::{
    app,
    solana::{
        accounts_helpers::fetch_program_config,
        constants::RPC_COMMITMENT,
        instructions::ZkPoolProgram,
        program_helpers::ID,
        tx_builder::send_v0,
    },
    state::{AppState, Config},
};

pub const LAMPORTS_PER_SOL: u64 = 1_000_000_000;
const DEFAULT_RPC_URL: &str = "http://127.0.0.1:8899";
const DEFAULT_WS_URL: &str = "ws://127.0.0.1:8900";

/// Throwaway key that initializes the pool on a surfnet and so becomes its owner.
const POOL_OWNER_KEY_JSON: &str = include_str!("../fixtures/keys/tmpo2qzmuksinGFdeEcYCySxtc2kwhzAxWy4y65dLcr.json");

pub fn pool_owner() -> Keypair {
    let bytes: Vec<u8> = serde_json::from_str(POOL_OWNER_KEY_JSON).unwrap();
    Keypair::try_from(bytes.as_slice()).unwrap()
}

pub struct SurfnetEnv {
    pub rpc_url: String,
    pub ws_url: String,
    pub rpc: Arc<RpcClient>,
    pub pool: ZkPoolProgram,
}

impl SurfnetEnv {
    /// Panics with a clear message if the surfnet is not running or the program is not deployed.
    pub async fn connect() -> Self {
        let rpc_url = std::env::var("SURFNET_RPC_URL").unwrap_or_else(|_| DEFAULT_RPC_URL.into());
        let ws_url = std::env::var("SURFNET_WS_URL").unwrap_or_else(|_| DEFAULT_WS_URL.into());
        let rpc = Arc::new(RpcClient::new_with_commitment(rpc_url.clone(), RPC_COMMITMENT));

        if let Err(e) = rpc.get_version().await {
            panic!("surfnet RPC at {rpc_url} does not answer ({e}); run `just it`");
        }
        let program = rpc
            .get_account_with_commitment(&ID, RPC_COMMITMENT)
            .await
            .unwrap_or_else(|e| panic!("cannot read program account {ID}: {e}"))
            .value;
        if !program.is_some_and(|account| account.executable) {
            panic!("program {ID} is not deployed on {rpc_url}; run `just it`");
        }

        let pool = ZkPoolProgram::new(anchor_client::Cluster::Custom(rpc_url.clone(), ws_url.clone())).unwrap();
        Self { rpc_url, ws_url, rpc, pool }
    }

    /// Sends `initialize`, signed by `pool_owner()`, if `program_config` is missing.
    /// Returns the owner.
    pub async fn ensure_pool_initialized(&self) -> Keypair {
        let owner = pool_owner();
        if fetch_program_config(&self.rpc).await.unwrap().is_none() {
            self.fund(&owner.pubkey(), 2 * LAMPORTS_PER_SOL).await;
            send_v0(&self.rpc, &owner, &[super::admin_ixs::initialize(&self.pool, owner.pubkey())])
                .await
                .expect("initialize");
        }
        owner
    }

    /// A fresh keypair holding `lamports`. Airdrop first; the `surfnet_setAccount` cheatcode
    /// if the airdrop fails.
    pub async fn funded_keypair(&self, lamports: u64) -> Keypair {
        let keypair = Keypair::new();
        self.fund(&keypair.pubkey(), lamports).await;
        keypair
    }

    pub async fn fund(&self, pubkey: &Pubkey, lamports: u64) {
        let balance_before = self.rpc.get_balance(pubkey).await.unwrap_or(0);
        if self.airdrop(pubkey, lamports).await.is_err() {
            self.surfnet_rpc("surfnet_setAccount", json!([pubkey.to_string(), { "lamports": balance_before + lamports }]))
                .await;
        }
        let balance = self.rpc.get_balance(pubkey).await.unwrap();
        assert!(balance >= balance_before + lamports, "funding {pubkey} failed: balance {balance}");
    }

    async fn airdrop(&self, pubkey: &Pubkey, lamports: u64) -> Result<(), String> {
        let signature = self.rpc.request_airdrop(pubkey, lamports).await.map_err(|e| e.to_string())?;
        for _ in 0..100 {
            if self.rpc.confirm_transaction(&signature).await.map_err(|e| e.to_string())? {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Err(format!("airdrop {signature} not confirmed"))
    }

    /// Raw JSON-RPC call, e.g. Surfpool cheatcodes (`surfnet_setAccount`).
    pub async fn surfnet_rpc(&self, method: &'static str, params: Value) -> Value {
        self.rpc
            .send::<Value>(RpcRequest::Custom { method }, params)
            .await
            .unwrap_or_else(|e| panic!("{method} failed: {e}"))
    }

    /// Starts the real server (`app()`, the same router `main.rs` serves) on a free local port,
    /// against the surfnet: one fresh, funded deposit wallet and one withdraw wallet, zk deposit
    /// data in a temp dir (kept alive by the returned `ZkPoolClientTestServer`).
    pub async fn start_server(&self) -> ZkPoolClientTestServer {
        let deposit_wallet = self.funded_keypair(50 * LAMPORTS_PER_SOL).await;
        let withdraw_wallet = self.funded_keypair(5 * LAMPORTS_PER_SOL).await;
        let zk_deposit_data_dir = tempfile::tempdir().unwrap();

        // mocked config for the server
        let config = Config {
            version: "it".into(),
            solana_rpc_url: self.rpc_url.clone(),
            solana_ws_url: self.ws_url.clone(),
            program_id: ID,
            deposit_wallet_keys: vec![Arc::new(deposit_wallet)],
            withdraw_wallet_keys: vec![Arc::new(withdraw_wallet)],
            zk_deposit_data_dir: zk_deposit_data_dir.path().to_path_buf(),
        };
        let state = AppState::new(config).unwrap();

        // Port 0: the OS picks a free port. The server task stops with the test's runtime.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());

        // start the test server in a new task
        tokio::spawn(axum::serve(listener, app(state.clone())).into_future());

        ZkPoolClientTestServer { state, base_url, client: reqwest::Client::new(), _zk_deposit_data_dir: zk_deposit_data_dir }
    }
}

pub struct ZkPoolClientTestServer {
    /// The state the server runs with (its wallets, its zk deposit data store).
    pub state: AppState,
    /// e.g. `http://127.0.0.1:54321`
    pub base_url: String,
    client: reqwest::Client,
    _zk_deposit_data_dir: TempDir,
}

impl ZkPoolClientTestServer {
    /// Returns the status and the JSON body (`Value::Null` for an empty body).
    pub async fn get(&self, path: &str) -> (StatusCode, Value) {
        response(self.client.get(self.url(path)).send().await.unwrap()).await
    }

    pub async fn post_json(&self, path: &str, body: Value) -> (StatusCode, Value) {
        response(self.client.post(self.url(path)).json(&body).send().await.unwrap()).await
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }
}

async fn response(response: reqwest::Response) -> (StatusCode, Value) {
    let status = StatusCode::from_u16(response.status().as_u16()).unwrap();
    let bytes = response.bytes().await.unwrap();
    let body = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
    (status, body)
}
