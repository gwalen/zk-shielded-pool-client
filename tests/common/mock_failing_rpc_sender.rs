use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use serde_json::Value;
use solana_rpc_client::{
    http_sender::HttpSender,
    rpc_sender::{RpcSender, RpcTransportStats},
};
use solana_rpc_client_api::{
    client_error::{ErrorKind, Result},
    request::RpcRequest,
};

pub const SIMULATED_RPC_FAILURE: &str = "simulated rpc failure";

/// Test-only RPC transport that makes chosen RPC methods fail, because Surfpool can't fail on demand.
/// Lets tests reach the path where a tx lands and a later RPC call fails: other calls go to the node,
/// sending `failing_request` returns an error until `failures_left` (decrementing counter) goes to 0.
pub struct MockFailingRpcSender {
    http_sender: HttpSender,
    failing_request: RpcRequest,
    failures_left: AtomicUsize,
}

impl MockFailingRpcSender {
    pub fn new(rpc_url: &str, failing_request: RpcRequest, expected_failure_count: usize) -> Self {
        Self {
            http_sender: HttpSender::new(rpc_url),
            failing_request,
            failures_left: AtomicUsize::new(expected_failure_count),
        }
    }
}

#[async_trait]
impl RpcSender for MockFailingRpcSender {
    async fn send(&self, request: RpcRequest, params: Value) -> Result<Value> {
        if request == self.failing_request {
            let failures_left_bigger_than_zero = self
                .failures_left
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |failures_left| {
                    failures_left.checked_sub(1)
                })
                .is_ok();
        
            if failures_left_bigger_than_zero {
                return Err(ErrorKind::Custom(SIMULATED_RPC_FAILURE.into()).into());
            }
        }
        self.http_sender.send(request, params).await
    }

    fn get_transport_stats(&self) -> RpcTransportStats {
        self.http_sender.get_transport_stats()
    }

    fn url(&self) -> String {
        self.http_sender.url()
    }
}
