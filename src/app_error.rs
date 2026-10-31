use axum::{
    Json,
    extract::rejection::JsonRejection,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use tracing::{error, warn};

/// Result for code that can run while serving an HTTP request: handlers, services, RPC/tx helpers
pub type RequestResult<T> = Result<T, AppError>;

/// Result for startup code that can't run while serving an HTTP request
pub type RawResult<T> = anyhow::Result<T>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Invalid input: {0}")]
    BadRequest(String),

    #[error("Rpc error: {0}")]
    RpcError(String),

    /// Preflight or on-chain failure: the tx did not take effect.
    #[error("Transaction rejected: {0}")]
    TxRejected(String),

    #[error(transparent)]
    Internal(#[from] anyhow::Error),

    #[error(transparent)]
    JsonError(#[from] JsonRejection),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Conflict: {0}")]
    Conflict(String),

    #[error("Prover error: {0}")]
    ProverError(String),
}

// simple struct to always print the same error format to the client
#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub error: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, public_error_msg) = match self {
            AppError::BadRequest(msg) => {
                warn!("Bad request: {msg:?}");
                (StatusCode::BAD_REQUEST, msg)
            }
            AppError::RpcError(rpc_error) => {
                error!("Rpc error: {rpc_error:?}");
                (StatusCode::INTERNAL_SERVER_ERROR, "rpc error".into())
            }
            AppError::TxRejected(tx_error) => {
                error!("Transaction rejected: {tx_error:?}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "transaction rejected".into(),
                )
            }
            AppError::Internal(error) => {
                error!("Internal error: {error:?}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal server error".into(),
                )
            }
            AppError::JsonError(json_rejection) => {
                warn!("Json error: {:?}", json_rejection.body_text());
                (json_rejection.status(), json_rejection.body_text())
            }
            AppError::NotFound(msg) => {
                warn!("Not found: {msg:?}");
                (StatusCode::NOT_FOUND, "not found".into())
            }
            AppError::Conflict(msg) => {
                warn!("Conflict: {msg:?}");
                (StatusCode::CONFLICT, "conflict".into())
            }
            AppError::ProverError(prover_error) => {
                error!("Prover error: {prover_error:?}");
                (StatusCode::INTERNAL_SERVER_ERROR, "prover error".into())
            }
        };
        (
            status,
            Json(ErrorBody {
                error: public_error_msg,
            }),
        )
            .into_response()
    }
}
