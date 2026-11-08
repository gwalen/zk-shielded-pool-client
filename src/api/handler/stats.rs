use axum::{Json, extract::State, http::StatusCode};

use crate::{api::data::stats_output::StatsOutput, app_error::RequestResult, state::AppState};

pub async fn handler(
    State(state): State<AppState>,
) -> RequestResult<(StatusCode, Json<StatsOutput>)> {
    let zk_deposit_data_list = state.zk_deposit_data_writer.fetch_all().await?;

    Ok((
        StatusCode::OK,
        Json(StatsOutput::from_zk_deposit_data_and_wallets(
            &zk_deposit_data_list,
            &state.deposit_wallets.pubkeys(),
            &state.withdraw_wallets.pubkeys(),
        )),
    ))
}
