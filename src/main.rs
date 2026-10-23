use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::EnvFilter;
use zk_shielded_pool_client::{
    app,
    state::{AppState, Config},
};


#[tokio::main]
async fn main() {
    // .env is optional: in prod the variables come from the real environment (Docker, k8s, ...)
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            // read RUST_LOG from environment if not set than throw error
            EnvFilter::try_from_default_env().expect("missing RUST_LOG env variable"),
        )
        .init();

    let config = Config::from_env().expect("invalid configuration");
    info!(?config, "Config loaded"); // as a separate field, keys are redacted

    let app_state = AppState::new(config).expect("failed to build app state");
    for pubkey in app_state.deposit_wallets.pubkeys() {
        info!(%pubkey, "Deposit wallet");
    }
    for pubkey in app_state.withdraw_wallets.pubkeys() {
        info!(%pubkey, "Withdraw wallet");
    }

    println!(
        "Available parallelism: {:?}",
        std::thread::available_parallelism()
    );
    println!(
        "Tokio workers: {}",
        tokio::runtime::Handle::current().metrics().num_workers()
    );

    let listener = TcpListener::bind("127.0.0.1:3000")
        .await
        .expect("failed to bind 127.0.0.1:3000");
    info!("Listening on http://{}", listener.local_addr().unwrap());

    axum::serve(listener, app(app_state)).await.expect("server error");
}
