use crate::{app_error::AppError, state::AppState};

pub async fn job(app_state: &AppState) -> Result<(), AppError> {
    tracing::info!(
        "--> Scheduler job running, rpc url {:?}",
        app_state.config.solana_rpc_url
    );
    Ok(())
}
