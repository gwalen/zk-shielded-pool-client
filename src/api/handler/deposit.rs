use crate::{
    api::data::{deposit_input::DepositInput, deposit_output::DepositOutput},
    app_error::AppError,
};
use axum::{Json, extract::rejection::JsonRejection, http::StatusCode};

pub async fn handler(
    // Json(input): Json<DepositInput>,
    payload: Result<Json<DepositInput>, JsonRejection>
) -> Result<(StatusCode, Json<DepositOutput>), AppError> {
    let Json(input) = payload?;
    println!("Deposit input: {:#?}", input);

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
