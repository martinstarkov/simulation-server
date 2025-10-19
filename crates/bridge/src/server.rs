use crate::{safe_block_on, service::ForwarderService};
use anyhow::Result;
use crossbeam_channel::{Receiver as CbReceiver, Select, Sender as CbSender};
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
    /// True if this client participates in step barriers
    pub step_participant: bool,

    /// Client -> server messages
    // Needs to be None for local clients so that their disconnect (drop) can be detected by the server.
    // If the server holds the channel, it never closes and so a local client will never fully disconnect.
    pub inbound_tx: Option<CbSender<ClientMsg>>,
    pub inbound_rx: CbReceiver<ClientMsg>,

    /// Server -> client messages
    pub outbound_tx: CbSender<ServerMsg>,
    pub outbound_rx: CbReceiver<ServerMsg>,
}

/// Shared state of the server
#[derive(Debug)]
pub struct Server<S: Simulation> {
    pub next_client_id: std::sync::atomic::AtomicU64,

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
        let server = Arc::new(Self {
            next_client_id: std::sync::atomic::AtomicU64::new(1),
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

    fn maybe_step_simulation(server: &Arc<Server<S>>, ready: &mut HashSet<u64>) {
        let total: usize = server.clients.iter().filter(|c| c.step_participant).count();
        ready.retain(|id| server.clients.contains_key(id));

        if total > 0 && ready.len() == total {
            println!(
                "[server] all {} participants ready, stepping simulation",
                total
            );

            if let Ok(mut sim) = server.sim.lock() {
                if let Err(e) = sim.step() {
                    eprintln!("Simulation step error: {e}");
                }

                let tick = sim.get_tick();
                let msg = ServerMsg {
                    body: Some(interface::server_msg::Body::Tick(tick)),
                };

                for entry in server.clients.iter() {
                    let _ = entry.outbound_tx.send(msg.clone());
                }
                ready.clear();
            }
        }
    }

    /// Start the synchronous simulation loop in a background thread.
    pub fn start(self: &Arc<Self>) -> thread::JoinHandle<Result<()>> {
        let server = self.clone();

        thread::spawn(move || -> Result<()> {
            println!("[server] simulation loop started");

            let mut ready = HashSet::new();

            // Cached state
            let mut cached_client_ids: Vec<u64> = Vec::new();
            let mut cached_receivers: Vec<CbReceiver<ClientMsg>> = Vec::new();
            let mut select = Select::new();

            loop {
                if server.shutdown_flag.load(Ordering::SeqCst) {
                    break;
                }

                // Collect and sort client IDs for stable ordering
                let mut current_ids: Vec<u64> = server.clients.iter().map(|c| *c.key()).collect();
                current_ids.sort_unstable();

                // Rebuild Select if clients changed
                if current_ids != cached_client_ids {
                    select = Select::new();
                    cached_receivers.clear();

                    for id in &current_ids {
                        if let Some(info) = server.clients.get(id) {
                            cached_receivers.push(info.inbound_rx.clone());
                        }
                    }

                    // Register all receivers *after* they are stored
                    for rx in &cached_receivers {
                        select.recv(rx);
                    }

                    cached_client_ids = current_ids;
                    if !cached_receivers.is_empty() {
                        println!(
                            "[server] select rebuilt for {} clients",
                            cached_receivers.len()
                        );
                    }
                }

                if cached_receivers.is_empty() {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }

                // === Wait for any client message (timeout to allow stepping) ===
                match select.select_timeout(Duration::from_millis(10)) {
                    Ok(oper) => {
                        let index = oper.index();
                        let client_id = cached_client_ids[index];
                        let rx = &cached_receivers[index];

                        match oper.recv(rx) {
                            Ok(msg) => match msg.body {
                                Some(interface::client_msg::Body::Shutdown(_)) => {
                                    println!("[server] client {} requested shutdown", client_id);
                                    break;
                                }
                                Some(interface::client_msg::Body::StepReady(_)) => {
                                    ready.insert(client_id);
                                    println!("[server] client {} ready", client_id);
                                }
                                _ => {
                                    if let Ok(mut sim) = server.sim.lock() {
                                        if let Ok(responses) = sim.handle_message(msg) {
                                            for entry in server.clients.iter() {
                                                for msg in &responses {
                                                    let _ = entry.outbound_tx.send(msg.clone());
                                                }
                                            }
                                        }
                                    }
                                }
                            },
                            Err(_) => {
                                println!("[server] client {} disconnected", client_id);
                                server.unregister_client(client_id);
                            }
                        }
                    }
                    Err(_) => {
                        // all receivers are gone
                        if server.clients.is_empty() {
                            println!("[server] all clients disconnected, shutting down");
                            break;
                        }
                    }
                }

                // === Check step barrier ===
                Self::maybe_step_simulation(&server, &mut ready);
            }

            // Cleanup
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

    pub fn register_client<const LOCAL: bool>(
        self: &Arc<Self>,
        step_participant: bool,
    ) -> (u64, CbSender<ClientMsg>, CbReceiver<ServerMsg>) {
        let (to_server_tx, to_server_rx) = crossbeam_channel::unbounded::<ClientMsg>();
        let (from_server_tx, from_server_rx) = crossbeam_channel::unbounded::<ServerMsg>();

        let client_id = self
            .next_client_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        self.clients.insert(
            client_id,
            ClientInfo {
                step_participant,
                inbound_tx: (!LOCAL).then_some(to_server_tx.clone()),
                inbound_rx: to_server_rx,
                outbound_tx: from_server_tx,
                outbound_rx: from_server_rx.clone(),
            },
        );

        println!(
            "[server] registered client {} (step_participant={})",
            client_id, step_participant
        );

        (client_id, to_server_tx, from_server_rx)
    }

    pub fn unregister_client(&self, client_id: u64) {
        if self.clients.remove(&client_id).is_some() {
            println!("[server] Unregistered client {}", client_id);
        }
    }
}
