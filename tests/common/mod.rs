// Each integration test binary uses a different subset of the harness.
#![allow(dead_code)]

pub mod admin_ixs;
pub mod mock_failing_rpc_sender;
pub mod surfnet;

/// KZG params the prover loads, fetched by `scripts/fetch-srs.sh`.
pub const SRS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/resources/kzg_bn254_16.srs");
