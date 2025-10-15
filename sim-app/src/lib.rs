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
use tokio::sync::{broadcast, mpsc, Notify};
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

#[derive(Default, Clone)]
struct ClientStatus {
    contributes: bool,
    voted_tick: Option<u64>,
}

pub enum CoreIn {
    FromClient(ClientMsg),
    AutoUnregister { app_id: String },
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
            let _ = l.send(msg.clone());
        }

        // Remote broadcast (async fan-out).
        if let Some(l) = &self.tx_out_remote {
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
            let _ = l.send(msg.clone());
        }
        if let Some(l) = &self.tx_out_remote {
            let _ = l.send(msg);
        }
    }

    fn apply_client(&mut self, msg: ClientMsg) {
        let app_id = msg.app_id.clone();
        match msg.body {
            Some(sim_proto::pb::sim::client_msg::Body::Register(Register { contributes })) => {
                if contributes {
                    print!(
                        "[Server] Registering contributing client: {}",
                        app_id.clone()
                    );
                    let entry = self.clients.entry(app_id.clone()).or_default();
                    entry.contributes = true;

                    // Immediately publish the current state so the joining contributor
                    // sees the tick it's expected to StepReady for (avoids deadlock).
                    self.broadcast_state(self.tick);
                } else {
                    self.clients.remove(&app_id);
                }
            }
            Some(sim_proto::pb::sim::client_msg::Body::StepReady(StepReady { tick })) => {
                if let Some(st) = self.clients.get_mut(&app_id) {
                    if st.contributes && tick == self.tick {
                        st.voted_tick = Some(tick);
                    }
                }
            }
            Some(sim_proto::pb::sim::client_msg::Body::Request(Req { id, name, data })) => {
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
            None => {}
        }
        self.notify.notify_waiters();
    }

    fn handle_inmsg(&mut self, inmsg: CoreIn) {
        match inmsg {
            CoreIn::FromClient(cm) => self.apply_client(cm),
            CoreIn::AutoUnregister { app_id } => {
                self.clients.remove(&app_id);
                self.notify.notify_waiters();
            }
        }
    }

    async fn drain_messages(&mut self) {
        // Drain all immediately available local messages first
        let mut rx_local_opt = self.rx_local.take();

        if let Some(ref mut rx_local) = rx_local_opt {
            while let Ok(msg) = rx_local.try_recv() {
                println!("Server is handling a local msg");
                self.handle_inmsg(msg);
            }
        }

        // Put it back
        self.rx_local = rx_local_opt;

        // Try to grab at most one remote async message
        if let Some(rx_remote) = &mut self.rx_remote {
            match tokio::time::timeout(Duration::from_millis(1), rx_remote.recv()).await {
                Ok(Some(msg)) => {
                    println!("Server is handling a remote msg");
                    self.handle_inmsg(msg);
                }
                _ => {} // timeout or channel closed — fine
            }
        }

        // Drain any newly arrived locals again (they're cheap)
        let mut rx_local_opt = self.rx_local.take();

        if let Some(ref mut rx_local) = rx_local_opt {
            while let Ok(msg) = rx_local.try_recv() {
                println!("Server is handling a local msg");
                self.handle_inmsg(msg);
            }
        }

        // Put it back
        self.rx_local = rx_local_opt;
    }

    async fn wait_for_cohort(&mut self, mut cohort: HashSet<String>) {
        loop {
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

            tokio::select! {
                _ = self.notify.notified() => {},
                _ = tokio::time::sleep(Duration::from_millis(2)) => {},
            }

            self.drain_messages().await;
        }
    }

    fn reset_votes(&mut self) {
        for st in self.clients.values_mut() {
            st.voted_tick = None;
        }
    }

    async fn advance_tick(&mut self, step_timer: &mut tokio::time::Interval) {
        step_timer.tick().await;
        self.tick += 1;
        println!("{} step", self.tick);
        self.broadcast_state(self.tick);
    }

    pub async fn run(mut self) {
        self.broadcast_state(self.tick);
        let mut step_timer = tokio::time::interval(STEP_INTERVAL);

        // Wait for at least one contributing client before ticking
        while self.cohort().is_empty() {
            println!("Server is awaiting for a client to connect");
            self.drain_messages().await;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        loop {
            self.drain_messages().await;

            let cohort = self.cohort();

            if !cohort.is_empty() {
                self.wait_for_cohort(cohort).await;

                self.reset_votes();
            }

            self.advance_tick(&mut step_timer).await;
        }
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
        let mut inbound = req.into_inner();

        let rx = self.bp.tx_out.subscribe();
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
            while let Some(item) = inbound.next().await {
                match item {
                    Ok(cm) => {
                        if !cm.app_id.is_empty() {
                            let mut g = app_id_holder.lock().await;
                            if g.is_none() {
                                *g = Some(cm.app_id.clone());
                            }
                        }
                        println!(
                            "[Server] received ClientMsg {:?}",
                            cm.body.as_ref().map(|b| std::mem::discriminant(b))
                        );
                        let _ = tx_in.send(CoreIn::FromClient(cm)).await;
                    }
                    Err(_status) => {
                        eprintln!("[Server] inbound error: {_status}");
                        break;
                    }
                }
            }
            println!("[Server] client stream ended");
            if let Some(id) = app_id_holder2.lock().await.clone() {
                let _ = tx_in.send(CoreIn::AutoUnregister { app_id: id }).await;
            }
        });

        Ok(Response::new(Box::pin(out_stream) as Self::LinkStream))
    }
}

/// Local app link (channels).

pub struct LocalAppLink {
    pub tx_in: channel::Sender<CoreIn>,
    pub rx_out: channel::Receiver<ServerMsg>,
}

impl LocalAppLink {
    pub fn send(&self, msg: ClientMsg) {
        let _ = self.tx_in.send(CoreIn::FromClient(msg));
    }
    pub fn next(&self) -> Option<ServerMsg> {
        self.rx_out.recv().ok()
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

fn start_simulator_core(core: SimulatorCore) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move { core.run().await })
}

async fn start_remote_service(
    addr: SocketAddr,
    tx_in: tokio::sync::mpsc::Sender<CoreIn>,
    tx_out: tokio::sync::broadcast::Sender<ServerMsg>,
) -> Result<()> {
    use tonic::transport::Server;

    let svc = SimulatorSvc {
        bp: Arc::new(Backplane { tx_in, tx_out }),
    };

    Server::builder()
        .add_service(SimulatorApiServer::new(svc))
        .serve(addr)
        .await
        .expect("gRPC service failed");

    eprintln!("[service] gRPC remote service started on {addr}");
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
pub fn spawn_local() -> Result<(LocalAppLink, std::thread::JoinHandle<()>)> {
    // Local channels
    let (tx_in_local, rx_in_local, tx_out_local, rx_out_local) = make_local_channels();

    // App link
    let link = LocalAppLink {
        tx_in: tx_in_local.clone(),
        rx_out: rx_out_local,
    };

    // Core (local only)
    let core = SimulatorCore::new(Some(rx_in_local), None, Some(tx_out_local.clone()), None);

    // Spawn in a background thread
    let join = thread::spawn(move || {
        block_on(async move {
            core.run().await;
        });
    });

    Ok((link, join))
}

/// Spawn a simulator locally with both in-process (Crossbeam) channels
/// and a gRPC remote service endpoint for external viewers/clients.

pub fn spawn_local_with_service(
    service_addr: SocketAddr,
) -> Result<(LocalAppLink, std::thread::JoinHandle<()>)> {
    // Local + remote channels
    let (tx_in_local, rx_in_local, tx_out_local, rx_out_local) = make_local_channels();
    let (tx_in_remote, rx_in_remote, tx_out_remote) = make_remote_channels();

    let link = LocalAppLink {
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

    // Spawn in a background thread
    let join = thread::spawn(move || {
        block_on(async move {
            let core_task = tokio::spawn(async move {
                core.run().await;
            });

            if let Err(e) = start_remote_service(service_addr, tx_in_remote, tx_out_remote).await {
                eprintln!("[Hybrid] failed to start gRPC service: {e}");
            }

            let _ = core_task.await;
        });
    });

    Ok((link, join))
}

/// Standalone gRPC simulator service (for remote mode).
pub async fn run_simulator_service(listen: SocketAddr) -> Result<()> {
    // === Remote-only channels ===
    let (tx_in_remote, rx_in_remote, tx_out_remote) = make_remote_channels();

    // === Spawn the simulator core ===
    let tx_out_for_core = tx_out_remote.clone();
    let core = SimulatorCore::new(None, Some(rx_in_remote), None, Some(tx_out_for_core));

    let join = start_simulator_core(core);

    // === Start gRPC service ===
    start_remote_service(listen, tx_in_remote, tx_out_remote).await?;

    tokio::select! {
        _ = join => eprintln!("[Remote] Simulator exited."),
        _ = tokio::signal::ctrl_c() => eprintln!("[Remote] Interrupted.")
    }

    Ok(())
}

/// Runs the remote simulator service in a blocking, synchronous context.
///
/// This starts a full simulator backend (core + gRPC API)
/// and blocks until interrupted or the simulator exits.
///
/// Safe to call from any thread, or from a synchronous test.
/// Does not require `#[tokio::main]` or `.await`.
pub fn run_simulator_service_blocking(listen: SocketAddr) -> Result<()> {
    let (tx_in_remote, rx_in_remote, tx_out_remote) = make_remote_channels();
    let tx_out_for_core = tx_out_remote.clone();
    let core = SimulatorCore::new(None, Some(rx_in_remote), None, Some(tx_out_for_core));

    block_on(async move {
        // Start gRPC service first
        let svc_task = tokio::spawn(start_remote_service(
            listen,
            tx_in_remote.clone(),
            tx_out_remote.clone(),
        ));

        // Wait briefly for service startup
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Start the simulator core
        core.run().await;

        svc_task.abort();
        Ok(())
    })
}
