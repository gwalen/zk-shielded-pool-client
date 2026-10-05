use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct HealthCheckOutput {
    pub msg: String,
    pub version: String,
}