pub mod api;
pub mod app_error;
pub mod solana;
pub mod state;
pub mod zk;

use api::handler::*;
use axum::{
    Router,
    extract::Request,
    routing::{get, post},
};
use state::AppState;
use tower::ServiceBuilder;
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, RequestId, SetRequestIdLayer},
    trace::TraceLayer,
};

/// Routes plus the middleware stack. `main` and the integration tests both use it, so the tests
/// run the real stack.
pub fn app(state: AppState) -> Router {
    Router::new()
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
        .with_state(state)
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
