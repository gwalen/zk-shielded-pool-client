//! Program events decoded from transaction logs.

use super::program_helpers::{ID, events};
use crate::app_error::{AppError, RequestResult};

pub use events::DepositDone;

/// All `DepositDone` events our program emitted in `logs`.
///
/// `parse_logs` tracks the invoke stack, so `Program data:` lines written by other programs
/// (even with a matching discriminator) are ignored.
pub fn parse_deposit_done(logs: &[String]) -> RequestResult<Vec<DepositDone>> {
    anchor_client::parse_logs::<DepositDone>(logs, &ID.to_string())
        .map_err(|e| AppError::RpcError(format!("cannot parse DepositDone logs: {e}")))
}
