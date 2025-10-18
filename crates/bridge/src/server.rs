use crate::{safe_block_on, service::ForwarderService};
use anyhow::Result;
use crossbeam_channel::{Receiver as CbReceiver, RecvTimeoutError, Sender as CbSender, unbounded};
use dashmap::DashMap;
use interface::{ClientMsg, ServerMsg, Simulation};
use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

/// Internal info tracked for each registered client.
#[derive(Debug, Clone)]
pub struct ClientInfo {
    pub step_participant: bool,
    pub outbound_tx: CbSender<ServerMsg>,
    pub outbound_rx: CbReceiver<ServerMsg>,
}

/// Shared state of the server
#[derive(Debug)]
pub struct Server<S: Simulation> {
    pub next_client_id: std::sync::atomic::AtomicU64,

    pub inbound_tx: CbSender<ClientMsg>,
    pub inbound_rx: CbReceiver<ClientMsg>,
    pub clients: DashMap<u64, ClientInfo>,
    pub sim: Arc<Mutex<S>>,
    shutdown_flag: Arc<AtomicBool>,
}

pub type SharedServer<S> = Arc<Server<S>>;

/// Server startup mode.
pub enum ServerMode {
    /// Start simulation only, no gRPC endpoint.
    Local,
    /// Start simulation + gRPC endpoint listening at the given address.
    WithGrpc(String),
}

impl<S: Simulation> Server<S> {
    pub fn new_with_grpc<StrType: Into<String>>(
        addr: StrType,
        sim: S,
    ) -> (SharedServer<S>, JoinHandle<Result<()>>) {
        Self::new(ServerMode::WithGrpc(addr.into()), sim)
    }

    pub fn new_local(sim: S) -> (SharedServer<S>, JoinHandle<Result<()>>) {
        Self::new(ServerMode::Local, sim)
    }

    /// Create a new server and start the simulation (and optionally gRPC).
    pub fn new(mode: ServerMode, sim: S) -> (SharedServer<S>, JoinHandle<Result<()>>) {
        let (in_tx, in_rx) = unbounded();
        let server = Arc::new(Self {
            next_client_id: std::sync::atomic::AtomicU64::new(1),
            inbound_tx: in_tx,
            inbound_rx: in_rx,
            clients: DashMap::new(),
            sim: Arc::new(Mutex::new(sim)),
            shutdown_flag: Arc::new(AtomicBool::new(false)),
        });

        let handle = server.start();

        // Optionally start gRPC endpoint.
        if let ServerMode::WithGrpc(addr) = mode {
            let grpc_ref = server.clone();
            std::thread::spawn(move || {
                safe_block_on(async move {
                    if let Err(e) = grpc_ref.start_grpc(&addr).await {
                        eprintln!("gRPC error: {:?}", e);
                    }
                });
            });
        }

        (server, handle)
    }

    pub fn shutdown(&self) {
        self.shutdown_flag.swap(true, Ordering::SeqCst);
    }

    /// Start the synchronous simulation loop in a background thread.
    fn start(self: &Arc<Self>) -> JoinHandle<Result<()>> {
        let server = self.clone();
        thread::spawn(move || -> Result<()> {
            println!("[server] simulation loop started");
            let mut ready = HashSet::new();

            loop {
                if server.shutdown_flag.load(Ordering::SeqCst) {
                    break;
                }

                // Process incoming messages
                let maybe_msg = match server.inbound_rx.recv_timeout(Duration::from_millis(1)) {
                    Ok(msg) => Some(msg),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(_) => break,
                };

                if let Some(msg) = maybe_msg {
                    match msg.body {
                        Some(interface::client_msg::Body::Shutdown(_)) => {
                            println!(
                                "[server] received shutdown request from client {}",
                                msg.client_id
                            );
                            break;
                        }
                        Some(interface::client_msg::Body::StepReady(_)) => {
                            ready.insert(msg.client_id);
                            println!("[server] client {} ready", msg.client_id);
                        }
                        _ => {
                            // Forward message to simulation
                            if let Ok(mut sim) = server.sim.lock() {
                                match sim.handle_message(msg) {
                                    Ok(responses) => {
                                        for entry in server.clients.iter() {
                                            for msg in &responses {
                                                let _ = entry.outbound_tx.send(msg.clone());
                                            }
                                        }
                                    }
                                    Err(e) => eprintln!("Simulation handle_message failed: {e}"),
                                }
                            }
                        }
                    }
                }

                // Barrier: wait until all step participants are ready
                let total: usize = server.clients.iter().filter(|c| c.step_participant).count();

                ready.retain(|id| server.clients.contains_key(id));

                if total > 0 && ready.len() == total {
                    println!(
                        "[server] all {} participants ready, stepping simulation",
                        total
                    );

                    // Step simulation
                    if let Ok(mut sim) = server.sim.lock() {
                        if let Err(e) = sim.step() {
                            eprintln!("Simulation step error: {e}");
                        }

                        // Get the new tick info
                        let tick = sim.get_tick();
                        let msg = ServerMsg {
                            body: Some(interface::server_msg::Body::Tick(tick)),
                        };

                        // Broadcast tick
                        for entry in server.clients.iter() {
                            let _ = entry.outbound_tx.send(msg.clone());
                        }

                        ready.clear();
                    }
                } else {
                    //println!("[server] {}/{} clients ready", ready.len(), total);
                }
            }

            server.clients.clear();

            println!("[server] shutdown");

            Ok(())
        })
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

    pub fn register_local_client(
        self: &Arc<Self>,
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

        let server_ref = self.clone();

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

                println!("[server] Client {} disconnected", client_id);
                server_ref.unregister_client(client_id);
            }
        });

        println!("[server] Registered client {}", client_id);

        (client_id, to_server_tx, from_server_rx)
    }

    pub fn unregister_client(&self, client_id: u64) {
        if self.clients.remove(&client_id).is_some() {
            println!("[server] Unregistered client {}", client_id);
        }
    }
}
