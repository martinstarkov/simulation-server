use crossbeam_channel::Receiver as CbReceiver;
use interface::{
    ClientMsg, RegisterRequest, RegisterResponse, ServerMsg, Simulation,
    forwarder_server::Forwarder,
};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};

use crate::server::SharedServer;

// -------------------- gRPC Service --------------------

#[derive(Clone)]
pub struct ForwarderService<S: Simulation> {
    pub server: SharedServer<S>,
}

#[tonic::async_trait]
impl<S: Simulation> Forwarder for ForwarderService<S> {
    type OpenStream = ReceiverStream<Result<ServerMsg, Status>>;

    // --- Registration RPC ---
    async fn register(
        &self,
        request: Request<RegisterRequest>,
    ) -> Result<Response<RegisterResponse>, Status> {
        let req = request.into_inner();

        // Create server↔client crossbeam channels
        let (outbound_tx, outbound_rx) = crossbeam_channel::unbounded::<ServerMsg>();

        // Allocate a new client ID atomically
        let client_id = self
            .server
            .next_client_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        self.server.clients.insert(
            client_id,
            crate::server::ClientInfo {
                step_participant: req.step_participant,
                outbound_tx: outbound_tx.clone(),
                outbound_rx: outbound_rx.clone(),
            },
        );

        println!(
            "[server] Registered remote client {} (step_participant={})",
            client_id, req.step_participant
        );

        Ok(Response::new(RegisterResponse { client_id }))
    }

    // --- Open bidirectional stream ---
    async fn open(
        &self,
        request: Request<Streaming<ClientMsg>>,
    ) -> Result<Response<Self::OpenStream>, Status> {
        // Extract client ID from gRPC metadata
        let md = request.metadata();
        let client_id: u64 = md
            .get("client-id")
            .ok_or_else(|| Status::unauthenticated("missing client-id metadata"))?
            .to_str()
            .map_err(|_| Status::invalid_argument("invalid client-id metadata"))?
            .parse()
            .map_err(|_| Status::invalid_argument("invalid client-id number"))?;

        println!("[server] Remote client {} opened stream", client_id);

        // --- Inbound: client → server ---
        let mut inbound = request.into_inner();
        let inbound_tx = self.server.inbound_tx.clone();
        let server_ref = self.server.clone();

        // Spawn background task for inbound forwarding
        tokio::spawn(async move {
            while let Some(result) = inbound.next().await {
                match result {
                    Ok(msg) => {
                        if inbound_tx.send(msg).is_err() {
                            break;
                        }
                    }
                    Err(_e) => {
                        //eprintln!("[server] client {} stream error: {:?}", client_id, e);
                        break;
                    }
                }
            }

            println!("[server] Client {} disconnected (inbound)", client_id);
            server_ref.unregister_client(client_id);
        });

        // --- Outbound: server → client ---
        let cb_rx: CbReceiver<ServerMsg> = match self.server.clients.get(&client_id) {
            Some(entry) => entry.outbound_rx.clone(),
            None => return Err(Status::not_found("client not registered")),
        };

        let (async_tx, async_rx) = tokio::sync::mpsc::channel::<Result<ServerMsg, Status>>(64);
        let server_ref = self.server.clone();

        std::thread::spawn(move || {
            for msg in cb_rx.iter() {
                if async_tx.blocking_send(Ok(msg)).is_err() {
                    break;
                }
            }

            println!("[server] Client {} disconnected (outbound)", client_id);
            server_ref.unregister_client(client_id);
        });

        Ok(Response::new(ReceiverStream::new(async_rx)))
    }
}
