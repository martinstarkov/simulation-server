use crossbeam_channel::{Receiver, Sender, unbounded};
use dashmap::DashMap;
use interface::{ClientMsg, ServerMsg};
use std::sync::Arc;

/// Synchronous message server using only crossbeam channels.
pub struct Server {
    inbound_tx: Sender<ClientMsg>,
    inbound_rx: Receiver<ServerMsg>,
    clients: Arc<DashMap<u64, Sender<ServerMsg>>>,
}

impl Server {
    /// Create a new server instance.
    pub fn new() -> Arc<Self> {
        let (tx_sv, rx_sv) = unbounded();
        let (tx_cl, rx_cl) = unbounded();
        Arc::new(Self {
            inbound_tx: tx_sv,
            inbound_rx: rx_cl,
            clients: Arc::new(DashMap::new()),
        })
    }

    /// Get the inbound sender for other components (e.g. gRPC service).
    pub fn inbound_tx(&self) -> Sender<ClientMsg> {
        self.inbound_tx.clone()
    }

    /// Blocking receive of messages from any client.
    pub fn recv(&self) -> ServerMsg {
        self.inbound_rx.recv().unwrap()
    }

    /// Register a client to receive responses from the server.
    pub fn register_client(&self, client_id: u64) -> Receiver<ServerMsg> {
        let (tx, rx) = unbounded();
        self.clients.insert(client_id, tx);
        rx
    }

    /// Unregister a client when it disconnects.
    pub fn unregister_client(&self, client_id: u64) {
        self.clients.remove(&client_id);
    }

    /// Send a message to a specific client.
    pub fn send_to_client(&self, client_id: u64, msg: ServerMsg) {
        if let Some(tx) = self.clients.get(&client_id) {
            let _ = tx.send(msg);
        }
    }

    /// Start the server’s main loop in a dedicated thread.
    /// For demo purposes, this just echoes back uppercase payloads.
    pub fn start(self: Arc<Self>) {
        todo!();
    }
}
