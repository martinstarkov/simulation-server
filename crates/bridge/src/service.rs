use interface::forwarder_server::{Forwarder, ForwarderServer};
use interface::{ClientMsg, RegisterRequest, RegisterResponse, ServerMsg};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use tokio_stream::Stream;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Server as GrpcServer;
use tonic::{Request, Response, Status};

use crate::server::Server;

/// Async tonic service that bridges gRPC messages into the synchronous `Server`.
#[derive(Clone)]
pub struct ForwarderSvc {
    server: Arc<Server>,
}

impl ForwarderSvc {
    pub fn new(server: Arc<Server>) -> Self {
        Self { server }
    }

    /// Start the gRPC server and serve this service.
    pub async fn run(self, addr: SocketAddr) {
        GrpcServer::builder()
            .add_service(ForwarderServer::new(self))
            .serve(addr)
            .await
            .unwrap();
    }
}

#[tonic::async_trait]
impl Forwarder for ForwarderSvc {
    type OpenStream = ReceiverStream<Result<ServerMsg, Status>>;

    async fn register(
        &self,
        _req: Request<RegisterRequest>,
    ) -> Result<Response<RegisterResponse>, Status> {
        let id = 0;
        Ok(Response::new(RegisterResponse { client_id: id }))
    }

    async fn open(
        &self,
        req: Request<tonic::Streaming<ClientMsg>>,
    ) -> Result<Response<Self::OpenStream>, Status> {
        let mut inbound = req.into_inner();

        let first = inbound
            .message()
            .await?
            .ok_or_else(|| Status::invalid_argument("expected first message"))?;
        let client_id = first.client_id.clone();

        let client_rx = self.server.register_client(client_id.clone());
        let inbound_tx = self.server.inbound_tx();
        let server_clone = self.server.clone();
        let cid_clone = client_id.clone();

        // Forward inbound messages -> Server
        tokio::spawn(async move {
            let _ = inbound_tx.send(first);
            while let Ok(Some(msg)) = inbound.message().await.map_err(|_| ()) {
                let _ = inbound_tx.send(msg);
            }
            server_clone.unregister_client(cid_clone);
        });

        // Bridge crossbeam (blocking) -> async via tokio::mpsc
        let (tx_async, rx_async) = tokio::sync::mpsc::channel::<Result<ServerMsg, Status>>(32);
        std::thread::spawn(move || {
            for msg in client_rx.iter() {
                let _ = tx_async.blocking_send(Ok(msg));
            }
        });

        Ok(Response::new(ReceiverStream::new(rx_async)))
    }
}
