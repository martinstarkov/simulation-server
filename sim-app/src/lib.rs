use anyhow::Result;
use crossbeam::channel;
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    time::Duration,
};

use futures_core::Stream;
use tokio::sync::{broadcast, mpsc, Notify};
use tokio_stream::{wrappers::BroadcastStream, StreamExt};
use tonic::{Request, Response, Status};

use sim_proto::pb::sim::{
    simulator_api_server::{SimulatorApi, SimulatorApiServer},
    Ack, ClientMsg, Register, Request as Req, ServerMsg, State, StepReady,
};

pub const STEP_INTERVAL: Duration = Duration::from_millis(10);
pub const CONTROL_INTERVAL: Duration = Duration::from_millis(500);
pub const STATE_WAIT_INTERVAL: Duration = Duration::from_millis(10000);

#[derive(Default, Clone)]
struct ClientStatus {
    contributes: bool,
    voted_tick: Option<u64>,
}

enum CoreIn {
    FromClient(ClientMsg),
    AutoUnregister { app_id: String },
}

/// Minimal simulator core with a blocking step cohort and generic requests.

pub struct SimulatorCore {
    rx_local: Option<crossbeam::channel::Receiver<CoreIn>>,
    rx_remote: tokio::sync::mpsc::Receiver<CoreIn>,
    tx_out_local: Option<crossbeam::channel::Sender<ServerMsg>>,
    tx_out_remote: tokio::sync::broadcast::Sender<ServerMsg>,

    tick: u64,
    clients: HashMap<String, ClientStatus>,
    notify: Arc<Notify>,
}

impl SimulatorCore {
    pub fn new(
        rx_local: Option<crossbeam::channel::Receiver<CoreIn>>,
        rx_remote: tokio::sync::mpsc::Receiver<CoreIn>,
        tx_out_local: Option<crossbeam::channel::Sender<ServerMsg>>,
        tx_out_remote: tokio::sync::broadcast::Sender<ServerMsg>,
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
        // Remote broadcast (async fan-out)
        let _ = self.tx_out_remote.send(msg);
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
        let _ = self.tx_out_remote.send(msg);
    }

    fn apply_client(&mut self, msg: ClientMsg) {
        let app_id = msg.app_id.clone();
        match msg.body {
            Some(sim_proto::pb::sim::client_msg::Body::Register(Register { contributes })) => {
                if contributes {
                    let entry = self.clients.entry(app_id.clone()).or_default();
                    entry.contributes = true;

                    // NEW: immediately publish the current state so the joining contributor
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
                self.handle_inmsg(msg);
            }
        }

        // Put it back
        self.rx_local = rx_local_opt;

        // Try to grab at most one remote async message
        match tokio::time::timeout(Duration::from_millis(1), self.rx_remote.recv()).await {
            Ok(Some(msg)) => self.handle_inmsg(msg),
            _ => {} // timeout or channel closed — fine
        }

        // Drain any newly arrived locals again (they're cheap)
        let mut rx_local_opt = self.rx_local.take();

        if let Some(ref mut rx_local) = rx_local_opt {
            while let Ok(msg) = rx_local.try_recv() {
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
        let mut out_stream = BroadcastStream::new(rx).map(|res| match res {
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
                        let _ = tx_in.send(CoreIn::FromClient(cm)).await;
                    }
                    Err(_status) => {
                        break;
                    }
                }
            }
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

/// Spawn simulator locally (channels), optionally also expose gRPC for remote viewers/clients.
pub async fn spawn_local(
    enable_remote_service: bool,
    service_addr: Option<SocketAddr>,
) -> Result<(LocalAppLink, tokio::task::JoinHandle<()>)> {
    use tonic::transport::Server;

    // === Local Crossbeam channels (for your in-process app) ===
    let (tx_in_local, rx_in_local) = crossbeam::channel::bounded::<CoreIn>(1024);
    let (tx_out_local, rx_out_local) = crossbeam::channel::bounded::<ServerMsg>(4096);

    // === Optional remote (Tokio) channels for gRPC viewer ===
    let (tx_in_remote, rx_in_remote) = tokio::sync::mpsc::channel::<CoreIn>(1024);
    let (tx_out_remote, _) = tokio::sync::broadcast::channel::<ServerMsg>(4096);

    // === Local app link (pure Crossbeam) ===
    let link = LocalAppLink {
        tx_in: tx_in_local.clone(),
        rx_out: rx_out_local,
    };

    // === Simulator core ===
    let core = if enable_remote_service {
        // Hybrid mode: core handles both Crossbeam + Tokio
        SimulatorCore::new(
            Some(rx_in_local),
            rx_in_remote,
            Some(tx_out_local.clone()),
            tx_out_remote.clone(),
        )
    } else {
        // Local-only mode: async channels not used
        let (_tx_in_remote_dummy, rx_in_remote_dummy) = tokio::sync::mpsc::channel::<CoreIn>(1);
        let (tx_out_remote_dummy, _) = tokio::sync::broadcast::channel::<ServerMsg>(1);

        SimulatorCore::new(
            Some(rx_in_local),
            rx_in_remote_dummy,
            Some(tx_out_local.clone()),
            tx_out_remote_dummy,
        )
    };

    // === Spawn the simulator core ===
    let join = tokio::spawn(async move { core.run().await });

    // === Optional gRPC viewer ===
    if enable_remote_service {
        let addr = service_addr.unwrap_or(([127, 0, 0, 1], 60000).into());
        let svc = SimulatorSvc {
            bp: Arc::new(Backplane {
                tx_in: tx_in_remote.clone(),
                tx_out: tx_out_remote.clone(),
            }),
        };

        tokio::spawn(async move {
            Server::builder()
                .add_service(SimulatorApiServer::new(svc))
                .serve(addr)
                .await
                .expect("gRPC service failed");
        });

        eprintln!("[local] gRPC viewer service on {addr}");
    }

    Ok((link, join))
}

/// Standalone gRPC simulator service (for remote mode).
pub async fn run_simulator_service(listen: SocketAddr) -> Result<()> {
    use tonic::transport::Server;

    let (tx_in, rx_in) = mpsc::channel::<CoreIn>(1024);
    let (tx_out_remote, _rx0) = broadcast::channel::<ServerMsg>(4096);

    // Clone BEFORE moving into the async task
    let tx_out_for_core = tx_out_remote.clone();

    tokio::spawn(async move {
        SimulatorCore::new(None, rx_in, None, tx_out_for_core)
            .run()
            .await;
    });

    let svc = SimulatorSvc {
        bp: Arc::new(Backplane {
            tx_in,
            tx_out: tx_out_remote, // still owned here
        }),
    };

    eprintln!("[remote] simulator gRPC server on {listen}");
    Server::builder()
        .add_service(SimulatorApiServer::new(svc))
        .serve(listen)
        .await?;

    Ok(())
}
