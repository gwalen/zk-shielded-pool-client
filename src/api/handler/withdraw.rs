use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
};
use tracing::info;

use crate::{
    api::data::{withdraw_input::WithdrawInput, withdraw_output::WithdrawOutput},
    app_error::RequestResult,
    service,
    state::AppState,
};

pub async fn handler(
    State(state): State<AppState>,
    payload: Result<Json<WithdrawInput>, JsonRejection>,
) -> RequestResult<Json<WithdrawOutput>> {
    let Json(input) = payload?;
    let request = input.into_request(&state.withdraw_wallets)?;
    let deposit_id = request.deposit_id.clone();
    let step = request.step;

    let receipt = service::withdraw::withdraw(&state, request).await?;
    info!(%deposit_id, step, withdraw_tx = %receipt.withdraw_tx, "Withdrawal confirmed");

    Ok(Json(WithdrawOutput::from(receipt)))
}
