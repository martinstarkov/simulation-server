use anyhow::Result;
use crossbeam::channel;
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    time::Duration,
};

use std::thread;

use futures_core::Stream;
use tokio::{
    sync::{broadcast, mpsc, Notify},
    task,
};
use tokio_stream::{wrappers::BroadcastStream, StreamExt};
use tonic::{Request, Response, Status};

use sim_proto::pb::sim::{
    simulator_api_server::{SimulatorApi, SimulatorApiServer},
    Ack, ClientMsg, Register, Request as Req, ServerMsg, State, StepReady,
};

pub mod client;

pub const STEP_INTERVAL: Duration = Duration::from_millis(10);
pub const CONTROL_INTERVAL: Duration = Duration::from_millis(500);
pub const STATE_WAIT_INTERVAL: Duration = Duration::from_millis(10000);

use tracing::{error, info};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

pub fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")); // default level
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(true).with_thread_ids(true))
        .init();
}

#[derive(Default, Clone)]
struct ClientStatus {
    contributes: bool,
    voted_tick: Option<u64>,
}

pub enum CoreIn {
    FromClient(ClientMsg),
    AutoUnregister { app_id: String },
    Shutdown,
}

/// Minimal simulator core with a blocking step cohort and generic requests.

pub struct SimulatorCore {
    // Optional local (crossbeam) input/output
    rx_local: Option<channel::Receiver<CoreIn>>,
    tx_out_local: Option<channel::Sender<ServerMsg>>,

    // Optional remote (Tokio) input/output
    rx_remote: Option<mpsc::Receiver<CoreIn>>,
    tx_out_remote: Option<broadcast::Sender<ServerMsg>>,

    tick: u64,
    clients: HashMap<String, ClientStatus>,
    notify: Arc<Notify>,

    running: bool,
}

impl SimulatorCore {
    pub fn new(
        rx_local: Option<channel::Receiver<CoreIn>>,
        rx_remote: Option<mpsc::Receiver<CoreIn>>,
        tx_out_local: Option<channel::Sender<ServerMsg>>,
        tx_out_remote: Option<broadcast::Sender<ServerMsg>>,
    ) -> Self {
        Self {
            rx_local,
            rx_remote,
            tx_out_local,
            tx_out_remote,
            tick: 0,
            clients: HashMap::new(),
            notify: Arc::new(Notify::new()),
            running: true,
        }
    }

    fn cohort(&self) -> HashSet<String> {
        self.clients
            .iter()
            .filter(|(_, st)| st.contributes)
            .map(|(id, _)| id.clone())
            .collect()
    }

    fn broadcast_state(&self, tick: u64) {
        let msg = ServerMsg {
            body: Some(sim_proto::pb::sim::server_msg::Body::State(State {
                tick,
                data: vec![],
            })),
        };

        // Local fast path (non-blocking)
        if let Some(l) = &self.tx_out_local {
            info!("[Server] Sending state to local clients");
            let _ = l.send(msg.clone());
        }

        // Remote broadcast (async fan-out).
        if let Some(l) = &self.tx_out_remote {
            info!("[Server] Sending state to remote clients");
            let _ = l.send(msg);
        }
    }

    fn send_ack(&self, id: u64, ok: bool, info: &str) {
        let msg = ServerMsg {
            body: Some(sim_proto::pb::sim::server_msg::Body::Ack(Ack {
                id,
                ok,
                info: info.to_string(),
            })),
        };
        if let Some(l) = &self.tx_out_local {
            info!("[Server] Sending ack to local clients");
            let _ = l.send(msg.clone());
        }
        if let Some(l) = &self.tx_out_remote {
            info!("[Server] Sending ack to remote clients");
            let _ = l.send(msg);
        }
    }

    fn apply_client(&mut self, msg: ClientMsg) {
        let app_id = msg.app_id.clone();
        match msg.body {
            Some(sim_proto::pb::sim::client_msg::Body::Register(Register { contributes })) => {
                if contributes {
                    info!(
                        "[Server] Registering contributing client: {}",
                        app_id.clone()
                    );
                    let entry = self.clients.entry(app_id.clone()).or_default();
                    entry.contributes = true;

                    // Immediately publish the current state so the joining contributor
                    // sees the tick it's expected to StepReady for (avoids deadlock).
                    self.broadcast_state(self.tick);
                } else {
                    info!("[Server] Removing contributing client: {}", app_id.clone());
                    self.clients.remove(&app_id);
                }
            }
            Some(sim_proto::pb::sim::client_msg::Body::StepReady(StepReady { tick })) => {
                if let Some(st) = self.clients.get_mut(&app_id) {
                    info!(
                        "[Server] Processing StepReady {} from client: {}",
                        tick,
                        app_id.clone()
                    );
                    if st.contributes {
                        if tick == self.tick {
                            info!("[Server] Setting client {} status to ready", app_id.clone());
                            st.voted_tick = Some(tick);
                        } else {
                            info!(
                                "[Server] Client {} has a unsynchronized tick: {}, server tick is {}",
                                app_id.clone(), tick, self.tick
                            );
                        }
                    } else {
                        info!(
                            "[Server] Client {} is not registered as contributing",
                            app_id.clone()
                        );
                    }
                }
            }
            Some(sim_proto::pb::sim::client_msg::Body::Request(Req { id, name, data })) => {
                info!(
                    "[Server] Processing client {} request to {}",
                    app_id.clone(),
                    name.as_str()
                );
                match name.as_str() {
                    // TODO: Add command processing here.
                    "set-thrust" => {
                        let _payload = data;
                        self.send_ack(id, true, "thrust set");
                    }
                    _ => {
                        self.send_ack(id, false, "unknown request");
                    }
                }
            }
            None => {
                info!(
                    "[Server] Unknown client message received from {}",
                    app_id.clone(),
                );
            }
        }
        self.notify.notify_waiters();
    }

    fn handle_inmsg(&mut self, inmsg: CoreIn) {
        match inmsg {
            CoreIn::FromClient(cm) => self.apply_client(cm),
            CoreIn::AutoUnregister { app_id } => {
                info!("[Server] Unregistering client {}", app_id.clone());
                self.clients.remove(&app_id);
                self.notify.notify_waiters();
            }
            CoreIn::Shutdown => {
                // graceful exit.
                self.running = false;
                self.notify.notify_waiters();
            }
        }
    }

    fn drain_messages(&mut self) {
        // 1) Drain all immediately-available local messages
        let mut rx_local_opt = self.rx_local.take();
        if let Some(ref mut rx_local) = rx_local_opt {
            while let Ok(msg) = rx_local.try_recv() {
                // debug!(...);
                self.handle_inmsg(msg);
            }
        }
        self.rx_local = rx_local_opt;

        let mut rx_remote_opt = self.rx_remote.take();
        // 2) Drain all immediately-available remote messages (tokio mpsc)
        if let Some(ref mut rx_remote) = rx_remote_opt {
            // tokio::sync::mpsc::Receiver::try_recv() is synchronous
            while let Ok(msg) = rx_remote.try_recv() {
                // debug!(...);
                self.handle_inmsg(msg);
            }
        }
        self.rx_remote = rx_remote_opt;

        // 3) Drain locals again (cheap)
        let mut rx_local_opt = self.rx_local.take();
        if let Some(ref mut rx_local) = rx_local_opt {
            while let Ok(msg) = rx_local.try_recv() {
                self.handle_inmsg(msg);
            }
        }
        self.rx_local = rx_local_opt;
    }

    fn wait_for_cohort(&mut self, mut cohort: HashSet<String>) {
        if !cohort.is_empty() {
            info!("[Server] Waiting for cohort...");
        }
        while self.running {
            let all_ready = cohort.iter().all(|id| {
                self.clients
                    .get(id)
                    .and_then(|c| c.voted_tick)
                    .map(|vt| vt == self.tick)
                    .unwrap_or(false)
            });
            if all_ready {
                break;
            }

            cohort.retain(|id| self.clients.get(id).map(|c| c.contributes).unwrap_or(false));
            if cohort.is_empty() {
                break;
            }

            self.drain_messages();

            thread::sleep(Duration::from_millis(2));
        }
        info!("[Server] Cohort ready!");
    }

    fn reset_votes(&mut self) {
        info!("[Server] Resetting contributing client statuses");

        for st in self.clients.values_mut() {
            st.voted_tick = None;
        }
    }

    fn advance_tick(&mut self) {
        self.tick += 1;
        info!("[Server] Advancing to tick: {}", self.tick);
        self.broadcast_state(self.tick);

        std::thread::sleep(std::time::Duration::from_micros(200));
    }

    pub fn update_loop(&mut self) {
        self.drain_messages();

        let cohort = self.cohort();

        if !cohort.is_empty() {
            self.wait_for_cohort(cohort);

            self.reset_votes();
        }

        self.advance_tick();
    }

    pub fn run(mut self) {
        info!("[Server] Starting...");
        self.broadcast_state(self.tick);

        // Wait for at least one contributing client before ticking
        while self.running && self.cohort().is_empty() {
            info!("[Server] Waiting for a client to connect...");
            self.drain_messages();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        while self.running {
            self.update_loop();
        }
        info!("[Server] Stopping...");
    }
}

#[derive(Clone)]
pub struct Backplane {
    pub tx_in: mpsc::Sender<CoreIn>,
    pub tx_out: broadcast::Sender<ServerMsg>,
}

#[derive(Clone)]
pub struct SimulatorSvc {
    bp: Arc<Backplane>,
}

#[tonic::async_trait]
impl SimulatorApi for SimulatorSvc {
    type LinkStream = Pin<Box<dyn Stream<Item = Result<ServerMsg, Status>> + Send + 'static>>;

    async fn link(
        &self,
        req: Request<tonic::Streaming<ClientMsg>>,
    ) -> Result<Response<Self::LinkStream>, Status> {
        // Stream of ClientMsg coming from the client.
        let mut inbound = req.into_inner();

        // Broadcast channel (tx_out) that the simulator core uses to publish ServerMsg to all connected clients.
        // subscribe gives this client its own receiver.
        let rx = self.bp.tx_out.subscribe();
        // wraps that receiver as a stream tonic can write to the network.
        let out_stream = BroadcastStream::new(rx).map(|res| match res {
            Ok(msg) => Ok(msg),
            Err(_lag) => Ok(ServerMsg {
                body: Some(sim_proto::pb::sim::server_msg::Body::State(State {
                    tick: 0,
                    data: b"lagged".to_vec(),
                })),
            }),
        });

        let tx_in = self.bp.tx_in.clone();

        let app_id_holder = Arc::new(tokio::sync::Mutex::new(None::<String>));
        let app_id_holder2 = app_id_holder.clone();

        tokio::spawn(async move {
            // Go through incoming client msgs and forward them to the server via the tx_in channel.
            while let Some(item) = inbound.next().await {
                match item {
                    Ok(cm) => {
                        if !cm.app_id.is_empty() {
                            let mut g = app_id_holder.lock().await;
                            if g.is_none() {
                                *g = Some(cm.app_id.clone());
                            }
                        }
                        info!("[API] Received ClientMsg");
                        let _ = tx_in.send(CoreIn::FromClient(cm)).await;
                    }
                    Err(_status) => {
                        info!("[API] Error: {_status}");
                        break;
                    }
                }
            }
            info!("[API] Client stream ended");
            if let Some(id) = app_id_holder2.lock().await.clone() {
                info!("[API] Sending unregister request");
                let _ = tx_in.send(CoreIn::AutoUnregister { app_id: id }).await;
            }
        });

        Ok(Response::new(Box::pin(out_stream) as Self::LinkStream))
    }
}

/// Local app link (channels).

pub struct LocalAppLink {
    app_id: String,
    pub tx_in: channel::Sender<CoreIn>,
    pub rx_out: channel::Receiver<ServerMsg>,
}

impl LocalAppLink {
    pub fn send(&self, msg: ClientMsg) {
        //info!("[Client: {}] Sending ClientMsg", self.app_id);
        let _ = self.tx_in.send(CoreIn::FromClient(msg));
    }
    pub fn next(&self) -> Option<ServerMsg> {
        let msg = self.rx_out.recv().ok();
        // if msg.is_some() {
        //     info!("[Client: {}] Receiving ServerMsg", self.app_id);
        // }
        msg
    }
    pub fn shutdown(&self) {
        let _ = self.tx_in.send(CoreIn::Shutdown);
    }
}

fn make_local_channels() -> (
    crossbeam::channel::Sender<CoreIn>,
    crossbeam::channel::Receiver<CoreIn>,
    crossbeam::channel::Sender<ServerMsg>,
    crossbeam::channel::Receiver<ServerMsg>,
) {
    use crossbeam::channel;
    let (tx_in, rx_in) = channel::bounded::<CoreIn>(1024);
    let (tx_out, rx_out) = channel::bounded::<ServerMsg>(4096);
    (tx_in, rx_in, tx_out, rx_out)
}

fn make_remote_channels() -> (
    tokio::sync::mpsc::Sender<CoreIn>,
    tokio::sync::mpsc::Receiver<CoreIn>,
    tokio::sync::broadcast::Sender<ServerMsg>,
) {
    use tokio::sync::{broadcast, mpsc};
    let (tx_in, rx_in) = mpsc::channel::<CoreIn>(1024);
    let (tx_out, _) = broadcast::channel::<ServerMsg>(4096);
    (tx_in, rx_in, tx_out)
}

async fn start_remote_service(
    app_id: Option<&str>,
    addr: SocketAddr,
    tx_in: tokio::sync::mpsc::Sender<CoreIn>,
    tx_out: tokio::sync::broadcast::Sender<ServerMsg>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    use tonic::transport::Server;

    let svc = SimulatorSvc {
        bp: Arc::new(Backplane { tx_in, tx_out }),
    };

    if let Some(app_id) = app_id {
        info!("[Client: {app_id}] gRPC remote service started on {addr}");
    } else {
        info!("[Server] gRPC remote service started on {addr}");
    }

    Server::builder()
        .add_service(SimulatorApiServer::new(svc))
        .serve_with_shutdown(addr, async move {
            let _ = shutdown.changed().await;
        })
        .await
        .expect("gRPC service failed");

    Ok(())
}

pub fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.block_on(fut)
    } else {
        let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
        rt.block_on(fut)
    }
}

/// Spawn a simulator locally using only in-process (Crossbeam) channels.
/// Fastest configuration; no remote gRPC service is started.
pub fn spawn_local(app_id: &str) -> Result<(LocalAppLink, std::thread::JoinHandle<()>)> {
    // Local channels
    let (tx_in_local, rx_in_local, tx_out_local, rx_out_local) = make_local_channels();

    // App link
    let link = LocalAppLink {
        app_id: app_id.to_string(),
        tx_in: tx_in_local.clone(),
        rx_out: rx_out_local,
    };

    // Core (local only)
    let core = SimulatorCore::new(Some(rx_in_local), None, Some(tx_out_local.clone()), None);

    let app_id_thread = app_id.to_string();

    // Spawn in a background thread
    let join = thread::spawn(move || {
        info!("[Client: {app_id_thread}] Starting server in local thread");
        core.run();
        info!("[Client: {app_id_thread}] Stopping server in local thread");
    });

    Ok((link, join))
}

/// Spawn a simulator locally with both in-process (Crossbeam) channels
/// and a gRPC remote service endpoint for external viewers/clients.

pub fn spawn_hybrid(
    app_id: &str,
    service_addr: SocketAddr,
) -> Result<(
    LocalAppLink,
    std::thread::JoinHandle<()>,
    std::thread::JoinHandle<()>,
    tokio::sync::watch::Sender<bool>,
)> {
    // Local + remote channels
    let (tx_in_local, rx_in_local, tx_out_local, rx_out_local) = make_local_channels();
    let (tx_in_remote, rx_in_remote, tx_out_remote) = make_remote_channels();

    let link = LocalAppLink {
        app_id: app_id.to_string(),
        tx_in: tx_in_local.clone(),
        rx_out: rx_out_local,
    };

    // Core handles both
    let core = SimulatorCore::new(
        Some(rx_in_local),
        Some(rx_in_remote),
        Some(tx_out_local.clone()),
        Some(tx_out_remote.clone()),
    );

    let app_id_thread = app_id.to_string();

    let core_handle = thread::spawn(move || {
        info!("[Client: {app_id_thread}] Starting server in local thread");
        core.run();
        info!("[Client: {app_id_thread}] Stopping server in local thread");
    });

    let app_id_thread2 = app_id.to_string();

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // Spawn in a background thread
    let svc_handle = thread::spawn(move || {
        block_on(async move {
            if let Err(e) = start_remote_service(
                Some(app_id_thread2.as_str()),
                service_addr,
                tx_in_remote,
                tx_out_remote,
                shutdown_rx,
            )
            .await
            {
                info!("[Client: {app_id_thread2}] Failed to start gRPC service: {e}");
            }
        });
    });

    Ok((link, core_handle, svc_handle, shutdown_tx))
}

/// Standalone gRPC simulator service (for remote mode).
pub async fn run_simulator_service(listen: SocketAddr) -> Result<()> {
    // === Remote-only channels ===
    let (tx_in_remote, rx_in_remote, tx_out_remote) = make_remote_channels();

    // === Spawn the simulator core on a blocking thread (sync run) ===
    let tx_out_for_core = tx_out_remote.clone();
    let core = SimulatorCore::new(None, Some(rx_in_remote), None, Some(tx_out_for_core));
    let core_handle = thread::spawn(move || {
        info!("[Server] Starting server in local thread");
        core.run();
        info!("[Server] Stopping server in local thread");
    });

    let (_, shutdown_rx) = tokio::sync::watch::channel(false);

    // === Start gRPC service on a Tokio task ===
    let svc_task = tokio::spawn({
        let tx_in = tx_in_remote.clone();
        let tx_out = tx_out_remote.clone();
        async move {
            info!("[Server] gRPC service starting on {}", listen);
            if let Err(e) = start_remote_service(None, listen, tx_in, tx_out, shutdown_rx).await {
                error!("[Server] gRPC service failed: {e}");
                Err(e)
            } else {
                info!("[Server] gRPC service finished");
                Ok::<(), anyhow::Error>(())
            }
        }
    });

    // Wrap the std thread join in spawn_blocking so we can await it
    let core_join = task::spawn_blocking(move || {
        core_handle
            .join()
            .map_err(|_| anyhow::anyhow!("Core thread panicked"))
    });

    // Race: service finishes, core exits, or ctrl+c
    tokio::select! {
        svc_res = svc_task => {
            // svc_task: Result<Result<(), anyhow::Error>, JoinError>
            svc_res??;
            info!("[Server] Service task done");
        }
        core_res = core_join => {
            // core_join: Result<Result<(), anyhow::Error>, JoinError>
            core_res??;
            info!("[Server] Core join done");
        }
        _ = tokio::signal::ctrl_c() => {
            info!("[Server] Interrupted (Ctrl+C)");
        }
    }

    Ok(())
}

// Blocking wrapper that runs the async service runner.
pub fn run_simulator_service_blocking(listen: SocketAddr) -> Result<()> {
    // Reuse your helper that either uses an existing runtime or creates one.
    block_on(async move {
        // This is your async orchestrator that starts:
        // - the core on a blocking thread
        // - the gRPC service on a Tokio task
        // and races them against ctrl_c()
        run_simulator_service(listen).await
    })
}
