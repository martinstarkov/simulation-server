use std::sync::Arc;

use crate::{client::Client, server::Server};
use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};
use interface::{ClientMsg, ClientMsgBody, ServerMsg};

/// Fully synchronous in-process client that communicates directly
/// with the `Server` via its crossbeam channels.
pub struct LocalClient {
    client_id: u64,
    to_server: Sender<ClientMsg>,
    from_server: Receiver<ServerMsg>,
}

impl LocalClient {
    /// Create a new local client connected directly to the given server.
    ///
    /// The client_id must be unique within the server.
    pub fn new(server: Arc<Server>, client_id: u64) -> Self {
        let to_server = server.inbound_tx();
        let from_server = server.register_client(client_id);
        Self {
            client_id,
            to_server,
            from_server,
        }
    }
}

impl Client for LocalClient {
    fn send(&self, body: ClientMsgBody) {
        let _ = self.to_server.send(ClientMsg {
            client_id: self.client_id,
            body: Some(body),
        });
    }

    fn recv(&self) -> Result<ServerMsg> {
        self.from_server
            .recv()
            .map_err(|e| anyhow::anyhow!("failed to receive from server: {}", e))
    }

    fn try_recv(&self) -> Option<ServerMsg> {
        self.from_server
            .try_recv()
            .map_err(|e| anyhow::anyhow!("failed to receive from server: {}", e))
            .ok()
    }
}
