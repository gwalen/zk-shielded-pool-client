use axum::{
    Json,
    extract::{Path, State},
};
use tracing::info;

use crate::{
    api::data::reconcile_deposit_output::ReconcileDepositOutput, app_error::RequestResult, service,
    state::AppState,
};

pub async fn handler(
    State(state): State<AppState>,
    Path(deposit_id): Path<String>,
) -> RequestResult<Json<ReconcileDepositOutput>> {
    let zk_deposit_data = service::deposit::reconcile_deposit(&state, &deposit_id).await?;
    info!(deposit_id = %zk_deposit_data.id, status = ?zk_deposit_data.status, "Deposit reconciled");

    Ok(Json(ReconcileDepositOutput::from(zk_deposit_data)))
}
