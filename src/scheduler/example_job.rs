
use std::time::Duration;

use crate::{app_error::AppError, state::AppState};

pub async fn example_job(_app_state: &AppState) -> Result<(), AppError> {
    loop {
        tracing::info!("--> Scheduler job running");
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}