use crate::{api::data::health_check_output::HealthCheckOutput, state::AppState};
use axum::{Json, extract::State, http::StatusCode};

pub async fn handler(State(app_state): State<AppState>) -> (StatusCode, Json<HealthCheckOutput>) {
    (
        StatusCode::OK,
        Json(HealthCheckOutput {
            msg: "OK".to_string(),
            version: app_state.config.version.clone(),
        }),
    )
}
