use axum::{Json, extract::rejection::JsonRejection, http::StatusCode, response::{IntoResponse, Response}};
use serde::Serialize;
use tracing::{error, warn};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Invalid input: {0}")]
    BadRequest(String),

    #[error("Rpc error: {0}")]
    RpcError(String),
    // RpcError(#[from] SomeRpcError),  // TODO: later use concrete error types like: solana_rpc_client_api::client_error::Error 

    #[error(transparent)]
    Internal(#[from] anyhow::Error),

    #[error(transparent)]
    JsonError(#[from] JsonRejection), 

    #[error("Scheduler error: {0}")]
    SchedulerError(String),

}

// simple struct to always print the same error format to the client
#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub error: String
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, public_error_msg) = match self {
            AppError::BadRequest(msg) => {
                warn!("Bad request: {msg:?}");
                (StatusCode::BAD_REQUEST, msg)
            },
            AppError::RpcError(rpc_error) => {
                error!("Rpc error: {rpc_error:?}");
                (StatusCode::INTERNAL_SERVER_ERROR, "rpc error".into())
            },
            AppError::Internal(error) => {
                error!("Internal error: {error:?}");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal server error".into())
            },
            AppError::JsonError(json_rejection) => {
                warn!("Json error: {:?}", json_rejection.body_text());
                (json_rejection.status(), json_rejection.body_text())
            },
            AppError::SchedulerError(scheduler_error) => {
                error!("Scheduler error: {scheduler_error:?}");
                (StatusCode::INTERNAL_SERVER_ERROR, "scheduler error".into())
            }
        };
        (status, Json(ErrorBody {error: public_error_msg})).into_response()
    }
}