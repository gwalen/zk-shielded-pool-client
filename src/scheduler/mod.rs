pub mod example_job;

use crate::{app_error::AppError, state::AppState};
use std::str::FromStr;

pub async fn run_scheduler(
    app_state: AppState,
    run_job: impl AsyncFn(&AppState) -> Result<(), AppError>,
) -> Result<(), AppError> {
    // todo!("run_scheduler");
    let schedule = cron::Schedule::from_str("0 */10 * * * *") // every 10 minutes
        .map_err(|e| AppError::SchedulerError(e.to_string()))?;
    loop {
        let next_run_time = schedule
            .upcoming(chrono::Utc)
            .next()
            .expect("schedule next failed");
        let wait_duration = (next_run_time - chrono::Utc::now()).to_std().unwrap(); // truncated to seconds
        tokio::time::sleep(wait_duration).await;
        if let Err(e) = run_job(&app_state).await {
            tracing::error!(error = ?e, "Job run failed");
        }
    }
}
