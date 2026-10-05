use axum::{Router, routing::{get, post}};
use tokio::net::TcpListener;

pub mod api;
pub mod state;
pub mod app_error;

use api::handler::*;
use state::*;

#[tokio::main]
async fn main() {
    // .env is optional: in prod the variables come from the real environment (Docker, k8s, ...)
    dotenvy::dotenv().ok();

    let config = Config::from_env().expect("invalid configuration");
    println!("Config: {config:?}");

    let app_state = AppState::new(config);

    println!(
        "Available parallelism: {:?}",
        std::thread::available_parallelism()
    );
    println!(
        "Tokio workers: {}",
        tokio::runtime::Handle::current().metrics().num_workers()
    );

    // TODO: add logger
    // TODO: add request id
    // TODO: scheduler for background tasks
    

    let app = Router::new()
        .route("/health", get(health_check::handler))
        // TODO: error handling from the handlers - I might want to log error with logger for monitoring
        //       but for client reutrn less information
        .route("/deposit", post(deposit::handler))
        // .route("/withdraw", get(withdraw::handler));
        .with_state(app_state);

    let listener = TcpListener::bind("127.0.0.1:3000")
        .await
        .expect("failed to bind 127.0.0.1:3000");
    println!("listening on http://{}", listener.local_addr().unwrap());

    axum::serve(listener, app).await.expect("server error");
}
