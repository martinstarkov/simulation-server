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

        // Reuse the unified registration logic
        let (client_id, _to_server, _from_server) =
            self.server.register_client::<false>(req.step_participant);

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
        let md = request.metadata();
        let client_id: u64 = md
            .get("client-id")
            .ok_or_else(|| Status::unauthenticated("missing client-id metadata"))?
            .to_str()
            .map_err(|_| Status::invalid_argument("invalid client-id metadata"))?
            .parse()
            .map_err(|_| Status::invalid_argument("invalid client-id number"))?;

        println!("[server] Remote client {} opened stream", client_id);

        let client = self
            .server
            .clients
            .get(&client_id)
            .ok_or_else(|| Status::not_found("client not registered"))?;

        let inbound_tx = client
            .inbound_tx
            .clone()
            .expect("Client inbound channel must be stored for remote case");
        let outbound_rx = client.outbound_rx.clone();
        drop(client);

        let mut inbound_stream = request.into_inner();
        let (async_tx, async_rx) = tokio::sync::mpsc::channel::<Result<ServerMsg, Status>>(128);
        let server_ref = self.server.clone();

        tokio::spawn(async move {
            loop {
                // Process inbound messages
                tokio::select! {
                    maybe_in = inbound_stream.next() => {
                        match maybe_in {
                            Some(Ok(mut msg)) => {
                                msg.client_id = client_id;
                                if inbound_tx.send(msg).is_err() {
                                    break;
                                }
                            }
                            Some(Err(e)) => {
                                eprintln!("[server] client {} inbound error: {:?}", client_id, e);
                                break;
                            }
                            None => break,
                        }
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(5)) => {
                        // Poll outbound messages periodically
                        while let Ok(msg) = outbound_rx.try_recv() {
                            if async_tx.send(Ok(msg)).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            }

            println!("[server] client {} disconnected", client_id);
            server_ref.unregister_client(client_id);
        });

        Ok(Response::new(ReceiverStream::new(async_rx)))
    }
}
