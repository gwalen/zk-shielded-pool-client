use crate::{
    api::data::{deposit_input::DepositInput, deposit_output::DepositOutput},
    app_error::RequestResult,
    service,
    state::AppState,
};
use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::StatusCode,
};
use tracing::info;

pub async fn handler(
    State(state): State<AppState>,
    payload: Result<Json<DepositInput>, JsonRejection>,
) -> RequestResult<(StatusCode, Json<DepositOutput>)> {
    let Json(input) = payload?;
    let request = input.into_request(&state.deposit_wallets)?;

    let receipt = service::deposit::deposit(&state, request).await?;
    info!(deposit_id = %receipt.zk_deposit_data.id, tx_signature = %receipt.tx_signature, "Deposit confirmed");

    Ok((StatusCode::OK, Json(DepositOutput::from(receipt))))
}
