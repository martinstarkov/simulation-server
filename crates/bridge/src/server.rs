use crate::service::ForwarderService;
use crossbeam_channel::{Receiver as CbReceiver, RecvTimeoutError, Sender as CbSender, unbounded};
use dashmap::DashMap;
use interface::{ClientMsg, ServerMsg, Tick};
use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

/// Internal info tracked for each registered client.
#[derive(Debug)]
pub struct ClientInfo {
    pub step_participant: bool,
    pub outbound_tx: CbSender<ServerMsg>,
    pub outbound_rx: CbReceiver<ServerMsg>,
}

/// Shared state of the server
#[derive(Debug)]
pub struct ServerState {
    pub next_client_id: std::sync::atomic::AtomicU64,

    pub inbound_tx: CbSender<ClientMsg>,
    pub inbound_rx: CbReceiver<ClientMsg>,
    pub clients: DashMap<u64, ClientInfo>,
}

pub type SharedServer = Arc<ServerState>;

impl ServerState {
    /// Build a fresh server and return an `Arc` to it.
    pub fn new() -> SharedServer {
        let (in_tx, in_rx) = unbounded();
        Arc::new(Self {
            next_client_id: std::sync::atomic::AtomicU64::new(1),
            inbound_tx: in_tx,
            inbound_rx: in_rx,
            clients: DashMap::new(),
        })
    }

    /// Start the synchronous simulation loop in a background thread.
    pub fn start_simulation(self: &Arc<Self>) {
        let server = self.clone();
        thread::spawn(move || run_simulation_loop(server));
    }

    /// Convenience for starting the gRPC server.
    pub async fn start_grpc(self: &Arc<Self>, addr: &str) -> anyhow::Result<()> {
        let service = ForwarderService {
            server: self.clone(),
        };
        let addr = addr.parse()?;
        tonic::transport::Server::builder()
            .add_service(interface::forwarder_server::ForwarderServer::new(service))
            .serve(addr)
            .await?;
        Ok(())
    }

    pub fn register_client(
        &self,
        step_participant: bool,
    ) -> (u64, CbSender<ClientMsg>, CbReceiver<ServerMsg>) {
        let (to_server_tx, to_server_rx) = unbounded::<ClientMsg>();
        let (from_server_tx, from_server_rx) = unbounded::<ServerMsg>();

        let client_id = self
            .next_client_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        self.clients.insert(
            client_id,
            ClientInfo {
                step_participant,
                outbound_tx: from_server_tx.clone(),
                outbound_rx: from_server_rx.clone(),
            },
        );

        // Spawn thread to forward from client → server inbound queue
        let inbound_tx = self.inbound_tx.clone();
        std::thread::spawn({
            let inbound_tx = inbound_tx.clone();
            move || {
                for mut msg in to_server_rx.iter() {
                    msg.client_id = client_id;
                    if inbound_tx.send(msg).is_err() {
                        break;
                    }
                }
                println!("[server] Local client {} disconnected", client_id);
            }
        });

        println!("[server] Registered local client {}", client_id);

        (client_id, to_server_tx, from_server_rx)
    }

    pub fn unregister_client(&self, client_id: u64) {
        if self.clients.remove(&client_id).is_some() {
            println!("[server] Client {} unregistered", client_id);
        }
    }
}

// -------------------- Simulation Loop --------------------

pub fn run_simulation_loop(server: SharedServer) {
    println!("[core] simulation loop started");
    let mut seq = 0u64;
    let start_time = Instant::now();

    loop {
        let maybe_msg = match server.inbound_rx.recv_timeout(Duration::from_millis(16)) {
            Ok(msg) => Some(msg),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => break,
        };

        if let Some(msg) = maybe_msg {
            println!("[core] received message from client {}", msg.client_id);
        }

        seq += 1;
        let tick = ServerMsg {
            body: Some(interface::server_msg::Body::Tick(Tick {
                seq,
                time_s: start_time.elapsed().as_secs_f32(),
            })),
        };

        for entry in server.clients.iter() {
            let _ = entry.outbound_tx.send(tick.clone());
        }

        std::thread::sleep(Duration::from_millis(16));
    }

    println!("[core] simulation loop stopped");
}
