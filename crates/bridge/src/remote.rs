use crate::client::Client;
use anyhow::Result;
use crossbeam_channel::{Receiver, Sender, unbounded};
use interface::{
    ClientMsg, ClientMsgBody, RegisterRequest, ServerMsg, forwarder_client::ForwarderClient,
};
use std::thread;
use tokio_stream::{StreamExt, wrappers::ReceiverStream};
use tonic::Request;

pub fn safe_block_on<F: std::future::Future>(fut: F) -> F::Output {
    if tokio::runtime::Handle::try_current().is_ok() {
        // Already inside a runtime — block in place
        tokio::task::block_in_place(|| futures::executor::block_on(fut))
    } else {
        // Create a lightweight, single-threaded runtime
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        rt.block_on(fut)
    }
}

/// RemoteClient that provides a *synchronous* API using crossbeam channels,
/// and a single background thread that forwards messages to/from gRPC.
pub struct RemoteClient {
    client_id: u64,
    to_server: Sender<ClientMsg>,
    from_server: Receiver<ServerMsg>,
}

impl RemoteClient {
    pub fn connect(addr: &str) -> anyhow::Result<Self> {
        // crossbeam channels for synchronous user API
        let (user_tx, user_rx) = unbounded::<ClientMsg>(); // user → server
        let (srv_tx, srv_rx) = unbounded::<ServerMsg>(); // server → user

        // Start background thread that owns a Tokio runtime
        let addr_str = addr.to_string();
        let srv_tx_clone = srv_tx.clone();

        thread::spawn(move || {
            safe_block_on(async move {
                // Connect to gRPC
                let mut grpc = ForwarderClient::connect(addr_str).await.unwrap();

                // Register and get client_id
                let resp = grpc
                    .register(Request::new(RegisterRequest {}))
                    .await
                    .unwrap();
                let client_id = resp.into_inner().client_id.clone();

                let (tx_async, rx_async) = tokio::sync::mpsc::channel::<ClientMsg>(32);
                let cid = client_id.clone();
                let user_rx_clone = user_rx.clone();

                // One blocking thread that forwards from crossbeam → async mpsc
                std::thread::spawn(move || {
                    for payload in user_rx_clone.iter() {
                        let _ = tx_async.blocking_send(payload);
                    }
                });

                let outbound = ReceiverStream::new(rx_async);

                // Start bidirectional stream
                let mut inbound = grpc
                    .open(Request::new(outbound))
                    .await
                    .unwrap()
                    .into_inner();

                // Receive responses from server and push into crossbeam channel
                while let Some(Ok(msg)) = inbound.next().await.map(|r| r.map_err(|_| ())) {
                    let _ = srv_tx_clone.send(msg);
                }
            });
        });

        Ok(Self {
            client_id: 0, // not needed externally
            to_server: user_tx,
            from_server: srv_rx,
        })
    }
}

impl Client for RemoteClient {
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
