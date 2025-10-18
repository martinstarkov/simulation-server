use crate::safe_block_on;
use crate::server::SharedServer;
use anyhow::Result;
use crossbeam_channel::{Receiver, Sender, unbounded};
use interface::{
    ClientMsg, ClientMsgBody, RegisterRequest, ServerMsg, forwarder_client::ForwarderClient,
};
use interface::{ServerMsgBody, Shutdown, Simulation, StepReady, Tick};
use std::str::FromStr;
use std::thread;
use std::time::Duration;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use tonic::Request;
use tonic::metadata::MetadataValue;

#[derive(Clone, Debug)]
pub struct Client {
    step_participant: bool,
    client_id: u64,
    to_server: Sender<ClientMsg>,
    from_server: Receiver<ServerMsg>,
}

impl Client {
    pub fn new_local<S: Simulation>(
        step_participant: bool,
        server: &SharedServer<S>,
    ) -> Result<Self> {
        let (client_id, to_server, from_server) = server.register_local_client(step_participant);
        Ok(Self {
            step_participant,
            client_id,
            to_server,
            from_server,
        })
    }

    /// Connect to a **remote** gRPC server at the given address.
    ///
    /// You can pass either:
    /// - `"127.0.0.1:50051"` → automatically becomes `"http://127.0.0.1:50051"`
    /// - `"http://127.0.0.1:50051"` → used as is
    /// Connect to a **remote** gRPC server at the given address
    /// Connect to a remote gRPC server and wait for registration.
    pub fn new_remote<StrType: Into<String>>(
        step_participant: bool,
        address: StrType,
    ) -> Result<Self> {
        let (user_tx, user_rx) = unbounded::<ClientMsg>(); // user → server
        let (srv_tx, srv_rx) = unbounded::<ServerMsg>(); // server → user

        let addr = address.into();

        let addr_str = if addr.starts_with("http://") || addr.starts_with("https://") {
            addr.to_string()
        } else {
            format!("http://{}", addr)
        };

        // Channel to send back the registered client_id from async thread
        let (id_tx, id_rx) = crossbeam_channel::bounded::<u64>(1);

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

                // Send client_id back to the blocking constructor
                let _ = id_tx.send(client_id);

                // Bridge: crossbeam user_rx → async mpsc
                let (tx_async, rx_async) = tokio::sync::mpsc::channel::<ClientMsg>(64);
                let user_rx_clone = user_rx.clone();
                std::thread::spawn(move || {
                    for mut msg in user_rx_clone.iter() {
                        msg.client_id = client_id;
                        if tx_async.blocking_send(msg).is_err() {
                            break;
                        }
                    }
                    //println!("[remote-client] input bridge closed");
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
                        Err(_status) => {
                            //eprintln!("[remote-client] stream error: {:?}", status);
                            break;
                        }
                    }
                }

                println!("[remote-client] disconnected (id={})", client_id);
                drop(srv_tx);
                Ok::<(), anyhow::Error>(())
            });
        });

        // Block here until we know the assigned client_id
        let client_id = id_rx
            .recv()
            .expect("failed to receive client_id from async thread");

        println!("[remote-client] ready (client_id={})", client_id);

        Ok(Self {
            step_participant,
            client_id,
            to_server: user_tx,
            from_server: srv_rx,
        })
    }

    pub fn step_ready(&self) -> Result<Tick> {
        self.send(ClientMsgBody::StepReady(StepReady {}))?;

        // Block until a Tick is received
        loop {
            let msg = self.recv()?;

            if let Some(ServerMsgBody::Tick(tick)) = msg.body {
                return Ok(tick);
            }
            // Otherwise, ignore and keep waiting
        }
    }

    pub fn send(&self, body: ClientMsgBody) -> Result<()> {
        self.to_server.send(ClientMsg {
            client_id: self.client_id,
            body: Some(body),
        })?;
        Ok(())
    }

    pub fn shutdown_server(&self) -> Result<()> {
        self.send(interface::ClientMsgBody::Shutdown(Shutdown {}))?;
        Ok(())
    }

    pub fn recv(&self) -> Result<ServerMsg> {
        match self.from_server.recv() {
            Ok(msg) => Ok(msg),
            Err(_) => {
                anyhow::bail!("[remote-client] server closed connection")
            }
        }
    }

    pub fn try_recv(&self) -> Option<ServerMsg> {
        self.from_server.try_recv().ok()
    }

    /// Returns a blocking iterator over incoming messages.
    /// Ends when the server disconnects.
    pub fn iter(&self) -> Box<dyn Iterator<Item = ServerMsg> + '_> {
        Box::new(self.from_server.iter().map(|msg| msg))
    }

    /// Returns a non-blocking iterator over currently available messages.
    pub fn try_iter(&self) -> Box<dyn Iterator<Item = ServerMsg> + '_> {
        Box::new(self.from_server.try_iter().map(|msg| msg))
    }

    /// Forward all inbound `ServerMsg` to the tx channel.
    pub fn forward_all(&self, tx: &Sender<ServerMsg>) -> Result<()> {
        loop {
            match self.try_recv() {
                Some(msg) => tx.send(msg)?,
                None => break,
            }
        }
        Ok(())
    }

    pub fn step_participant(&self) -> bool {
        self.step_participant
    }

    pub fn id(&self) -> u64 {
        self.client_id
    }
}

fn spawn_step_thread(work_ms: u64, sim: Client) -> thread::JoinHandle<Result<()>> {
    thread::spawn(move || {
        loop {
            sim.step_ready()?;
            std::thread::sleep(Duration::from_millis(work_ms));
        }
    })
}

/// Connect a thread which just calls StepReady on the remote server, acting as a contributing client with no "work".
pub fn connect_remote_stepper<S: Into<String>>(
    addr: S,
    work_ms: u64,
) -> thread::JoinHandle<Result<()>> {
    let remote_client = Client::new_remote(true, addr);

    spawn_step_thread(work_ms, remote_client.expect("connect to remote failed"))
}
