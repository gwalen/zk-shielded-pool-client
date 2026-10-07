use axum::{
    Router,
    http::StatusCode,
    routing::{get, post},
};
use tokio::net::TcpListener;

pub mod api;
pub mod app_error;
pub mod state;
pub mod scheduler;

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

use crate::scheduler::{example_job, run_scheduler};

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

    // TODO: scheduler for background tasks
    //  - health check if scheduler is running
    //  - add info that scheduler is just a very basic solution that does not scale and while server 
    //    should be stateless so that there can be multiple instances of it, with such scheduler you would have
    //    duplicated scheduler logic running. On prod you should have a dedicated scheduler service integrated with cloud provider.

    // Task A: run the scheduler
    // let app_state_clone = app_state.clone();
    let scheduler = tokio::spawn(
        run_scheduler(app_state.clone(), example_job::job)    
    );
    // Task B: watcher task checking if scheduler was not aborted or panicked
    tokio::spawn(async move {
        if let Err(e) = scheduler.await {
            tracing::error!(error = ?e, "Scheduler task failed: {e:?}");
        }
    });


    let app = Router::new()
        .route("/health", get(health_check::handler))
        .route("/deposit", post(deposit::handler))
        // .route("/withdraw", get(withdraw::handler));
        .route("/boom", get(|| async { StatusCode::INTERNAL_SERVER_ERROR })) // test ERROR 500 response
        .layer(
            // NOTE: order of layers matters ! SetRequestIdLayer must be first, PropagateRequestIdLayer must be last.
            ServiceBuilder::new()
                .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
                .layer(
                    TraceLayer::new_for_http()
                        .make_span_with(create_request_span),
                    // -> This you add only if you want tracing to be reduced to 1 line instead of 3 lines per request
                    // .on_request(())
                    // .on_response(DefaultOnResponse::new().level(tracing::Level::INFO))
                    // .on_eos(())
                )
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

fn create_request_span(req: &Request) -> tracing::Span {
    // create request-id based on extensions
    let request_id = req
        .extensions()
        .get::<RequestId>() // set by SetRequestIdLayer, creates struct RequestId(HeaderValue), value is UUID
        .and_then(|request_id| request_id.header_value().to_str().ok()) // take Option(request_id) and convert to Option(String) ; and_than() is flatMap
        .unwrap_or("-empty request id- "); // if Option is None, return "-empty request id-"
    // build custom span with request-id
    tracing::info_span!(
        "request", 
        request_id = request_id, 
        method = %req.method(), 
        path = %req.uri().path(),
        version = ?req.version()
    )
}
