use crate::client::Client;
use anyhow::{Result, anyhow};
use crossbeam_channel::{Receiver, Sender, unbounded};
use interface::{
    ClientMsg, ClientMsgBody, RegisterRequest, ServerMsg, forwarder_client::ForwarderClient,
};
use std::str::FromStr;
use std::thread;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use tonic::Request;
use tonic::metadata::MetadataValue;

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

pub struct RemoteClient {
    to_server: Sender<ClientMsg>,
    from_server: Receiver<ServerMsg>,
}

impl RemoteClient {
    /// Connect to a **remote** gRPC server at the given address.
    ///
    /// You can pass either:
    /// - `"127.0.0.1:50051"` → automatically becomes `"http://127.0.0.1:50051"`
    /// - `"http://127.0.0.1:50051"` → used as is
    /// Connect to a **remote** gRPC server at the given address
    pub fn new(step_participant: bool, addr: &str) -> Result<Self> {
        let (user_tx, user_rx) = unbounded::<ClientMsg>(); // user → server
        let (srv_tx, srv_rx) = unbounded::<ServerMsg>(); // server → user

        let addr_str = if addr.starts_with("http://") || addr.starts_with("https://") {
            addr.to_string()
        } else {
            format!("http://{}", addr)
        };

        // Background thread that owns the async runtime
        thread::spawn(move || {
            let _ = safe_block_on(async move {
                let mut grpc = ForwarderClient::connect(addr_str)
                    .await
                    .expect("connect failed");

                // Register with the server → server assigns client_id
                let resp = grpc
                    .register(Request::new(RegisterRequest { step_participant }))
                    .await
                    .expect("register failed")
                    .into_inner();

                let client_id = resp.client_id;
                println!("[remote-client] registered with id {}", client_id);

                // Bridge: crossbeam user_rx → async mpsc
                let (tx_async, rx_async) = tokio::sync::mpsc::channel::<ClientMsg>(64);
                let user_rx_clone = user_rx.clone();
                std::thread::spawn(move || {
                    for mut msg in user_rx_clone.iter() {
                        // Fill in client_id automatically before sending
                        msg.client_id = client_id;
                        if tx_async.blocking_send(msg).is_err() {
                            break;
                        }
                    }
                    println!("[remote-client] input bridge closed");
                });

                let outbound = ReceiverStream::new(rx_async);

                // Attach client_id metadata
                let mut req = Request::new(outbound);
                req.metadata_mut().insert(
                    "client-id",
                    MetadataValue::from_str(&client_id.to_string()).unwrap(),
                );

                // Open the bidirectional stream
                let mut inbound = grpc.open(req).await.unwrap().into_inner();

                // Bridge inbound messages (server → client)
                while let Some(result) = inbound.next().await {
                    match result {
                        Ok(msg) => {
                            let _ = srv_tx.send(msg);
                        }
                        Err(status) => {
                            eprintln!("[remote-client] stream error: {:?}", status);
                            break;
                        }
                    }
                }

                println!("[remote-client] disconnected (id={})", client_id);
                Ok::<(), anyhow::Error>(())
            });
        });

        Ok(Self {
            to_server: user_tx,
            from_server: srv_rx,
        })
    }
}

impl Client for RemoteClient {
    fn send(&self, body: ClientMsgBody) {
        let _ = self.to_server.send(ClientMsg {
            client_id: 0, // Filled automatically by forwarder service.
            body: Some(body),
        });
    }

    fn recv(&self) -> Result<ServerMsg> {
        self.from_server
            .recv()
            .map_err(|e| anyhow!("receive failed: {}", e))
    }

    fn try_recv(&self) -> Option<ServerMsg> {
        self.from_server.try_recv().ok()
    }

    fn iter(&self) -> Box<dyn Iterator<Item = ServerMsg> + '_> {
        Box::new(self.from_server.iter().map(|msg| msg))
    }

    fn try_iter(&self) -> Box<dyn Iterator<Item = ServerMsg> + '_> {
        Box::new(self.from_server.try_iter().map(|msg| msg))
    }
}
