//! Client side of `comm-admin/1`.

use std::time::Duration;

use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr, SecretKey};
use thiserror::Error;

use crate::protocol::{AdminError, AdminOp, Reply, Request, ALPN, MAX_REPLY_BYTES};

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("could not start the admin client: {0}")]
    Bind(String),
    #[error(
        "could not reach the node (is the id/address right, and is this key an operator?): {0}"
    )]
    Connect(String),
    #[error("transport error: {0}")]
    Io(String),
    #[error("the node sent an invalid reply")]
    Protocol,
    #[error("the node refused: {0}")]
    Remote(AdminError),
    #[error("timed out")]
    Timeout,
}

pub struct AdminClient {
    endpoint: Endpoint,
}

impl AdminClient {
    /// `discovery` lets the client look the node up by id alone (needs
    /// network access to Iroh's discovery); without it, pass a full
    /// [`EndpointAddr`] with direct addresses.
    pub async fn new(secret: SecretKey, discovery: bool) -> Result<Self, ClientError> {
        let b = if discovery {
            Endpoint::builder(presets::N0)
        } else {
            Endpoint::builder(presets::Minimal)
        };
        let endpoint = b
            .secret_key(secret)
            .bind()
            .await
            .map_err(|e| ClientError::Bind(e.to_string()))?;
        Ok(Self { endpoint })
    }

    /// One request, a fresh random id.
    pub async fn request(&self, node: EndpointAddr, op: AdminOp) -> Result<Vec<u8>, ClientError> {
        let mut id = [0u8; 16];
        getrandom::fill(&mut id).map_err(|e| ClientError::Io(e.to_string()))?;
        self.request_with_id(node, u128::from_be_bytes(id), op)
            .await
    }

    /// Retry-safe form: re-sending the **same** `id` after a lost reply makes
    /// the node replay the stored outcome instead of running the operation
    /// twice.
    pub async fn request_with_id(
        &self,
        node: EndpointAddr,
        id: u128,
        op: AdminOp,
    ) -> Result<Vec<u8>, ClientError> {
        let work = async {
            let conn = self
                .endpoint
                .connect(node, ALPN)
                .await
                .map_err(|e| ClientError::Connect(e.to_string()))?;
            let (mut send, mut recv) = conn
                .open_bi()
                .await
                .map_err(|e| ClientError::Io(e.to_string()))?;
            let bytes =
                postcard::to_allocvec(&Request { id, op }).map_err(|_| ClientError::Protocol)?;
            send.write_all(&bytes)
                .await
                .map_err(|e| ClientError::Io(e.to_string()))?;
            send.finish().map_err(|e| ClientError::Io(e.to_string()))?;
            let reply = recv
                .read_to_end(MAX_REPLY_BYTES)
                .await
                .map_err(|e| ClientError::Io(e.to_string()))?;
            conn.close(0u32.into(), b"done");
            let (r, rest) =
                postcard::take_from_bytes::<Reply>(&reply).map_err(|_| ClientError::Protocol)?;
            if !rest.is_empty() || r.id != id {
                return Err(ClientError::Protocol);
            }
            r.result.map_err(ClientError::Remote)
        };
        tokio::time::timeout(Duration::from_secs(150), work)
            .await
            .map_err(|_| ClientError::Timeout)?
    }

    pub async fn close(self) {
        self.endpoint.close().await;
    }
}
