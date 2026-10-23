use crate::{
    api::data::{deposit_input::DepositInput, deposit_output::DepositOutput},
    app_error::{AppError, RequestResult},
};
use axum::{Json, extract::rejection::JsonRejection, http::StatusCode};
use tracing::info;

pub async fn handler(
    payload: Result<Json<DepositInput>, JsonRejection>
) -> RequestResult<(StatusCode, Json<DepositOutput>)> {
    let Json(input) = payload?;
    info!("Deposit input: {input:#?}");

    if input.deposit_wallet_address.is_empty() {
        return Err(AppError::BadRequest(
            "Deposit wallet address is empty".into(),
        ));
    }

    Ok((
        StatusCode::OK,
        Json(DepositOutput {
            tx_hash: "0x1234567890".to_string(),
        }),
    ))
}
