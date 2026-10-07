use axum::{
    Router,
    routing::{get, post},
};
use tokio::net::TcpListener;

pub mod api;
pub mod app_error;
pub mod state;

use api::handler::*;
use axum::extract::Request;
use state::*;
use tower::ServiceBuilder;
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, RequestId, SetRequestIdLayer},
    trace::TraceLayer,
};
use tracing::info;
use tracing_subscriber::EnvFilter;


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


    let app = Router::new()
        .route("/health", get(health_check::handler))
        .route("/deposit", post(deposit::handler))
        // .route("/withdraw", get(withdraw::handler));
        .layer(
            ServiceBuilder::new()
                .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
                .layer(
                    TraceLayer::new_for_http()
                        .make_span_with(create_request_span),
                )
                .layer(PropagateRequestIdLayer::x_request_id()),
        )
        .with_state(app_state);

    let listener = TcpListener::bind("127.0.0.1:3000")
        .await
        .expect("failed to bind 127.0.0.1:3000");
    info!("Listening on http://{}", listener.local_addr().unwrap());
    
    axum::serve(listener, app).await.expect("server error");
}

fn create_request_span(req: &Request) -> tracing::Span {
    // create request-id based on extensions
    let request_id = req
        .extensions()
        .get::<RequestId>() // set by SetRequestIdLayer, creates struct RequestId(HeaderValue), value is UUID
        .and_then(|request_id| request_id.header_value().to_str().ok())
        .unwrap_or("-empty request id- ");
    // build custom span with request-id
    tracing::info_span!(
        "request", 
        request_id = request_id, 
        method = %req.method(), 
        path = %req.uri().path(),
        version = ?req.version()
    )
}
