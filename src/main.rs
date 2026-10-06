use axum::{Router, http::StatusCode, routing::{get, post}};
use tokio::net::TcpListener;

pub mod api;
pub mod state;
pub mod app_error;

use api::handler::*;
use state::*;
use tower::ServiceBuilder;
use tower_http::{request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer}, trace::TraceLayer};
use tracing::info;
use tracing_subscriber::EnvFilter;
use axum::extract::Request;

#[tokio::main]
async fn main() {
    // .env is optional: in prod the variables come from the real environment (Docker, k8s, ...)
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt().with_env_filter(
        // read RUST_LOG from environment if not set than throw error
        EnvFilter::try_from_default_env().expect("missing RUST_LOG env variable")
    ).init();

    let config = Config::from_env().expect("invalid configuration");
    // info!("Config: {config:?}");     // inside the message text
    info!(?config, "Config loaded"); // as a separate field

    let app_state = AppState::new(config);

    println!(
        "Available parallelism: {:?}",
        std::thread::available_parallelism()
    );
    println!(
        "Tokio workers: {}",
        tokio::runtime::Handle::current().metrics().num_workers()
    );

    // TODO: add request id
    // TODO: scheduler for background tasks
    

    let app = Router::new()
        .route("/health", get(health_check::handler))
        .route("/deposit", post(deposit::handler))
        // .route("/withdraw", get(withdraw::handler));
        .route("/boom", get(|| async { StatusCode::INTERNAL_SERVER_ERROR })) // test ERROR response
        .layer(
            // NOTE: order of layers matters ! SetRequestIdLayer must be first, PropagateRequestIdLayer must be last.
            ServiceBuilder::new()
                .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
                .layer(TraceLayer::new_for_http() /* ...your one-line config... */)
                .layer(PropagateRequestIdLayer::x_request_id()),
        )
        .with_state(app_state);

    let listener = TcpListener::bind("127.0.0.1:3000")
        .await
        .expect("failed to bind 127.0.0.1:3000");
    info!("Listening on http://{}", listener.local_addr().unwrap());
    // Inline arguments ({name}) accept only a plain identifier, never an expression
    // A method call like listener.local_addr().unwrap() or a field access like config.version isn't allowed inside the braces.
    // info!("Listening on http://{listener.local_addr().unwrap():?}", ); // THIS WON'T WORK (read above explanation)

    axum::serve(listener, app).await.expect("server error");
}
