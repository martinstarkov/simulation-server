use anyhow::Result;
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
    rx: mpsc::Receiver<CoreIn>,
    tx_out: broadcast::Sender<ServerMsg>,

    tick: u64,
    clients: HashMap<String, ClientStatus>,
    notify: Arc<Notify>,
}

impl SimulatorCore {
    pub fn new(rx: mpsc::Receiver<CoreIn>, tx_out: broadcast::Sender<ServerMsg>) -> Self {
        Self {
            rx,
            tx_out,
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
        let _ = self.tx_out.send(ServerMsg {
            body: Some(sim_proto::pb::sim::server_msg::Body::State(State {
                tick,
                data: vec![],
            })),
        });
    }

    fn send_ack(&self, id: u64, ok: bool, info: &str) {
        let _ = self.tx_out.send(ServerMsg {
            body: Some(sim_proto::pb::sim::server_msg::Body::Ack(Ack {
                id,
                ok,
                info: info.to_string(),
            })),
        });
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

    pub async fn run(mut self) {
        self.broadcast_state(self.tick);
        let mut step_timer = tokio::time::interval(STEP_INTERVAL);

        loop {
            while let Ok(Some(inmsg)) =
                tokio::time::timeout(Duration::from_millis(1), self.rx.recv()).await
            {
                match inmsg {
                    CoreIn::FromClient(cm) => self.apply_client(cm),
                    CoreIn::AutoUnregister { app_id } => {
                        self.clients.remove(&app_id);
                        self.notify.notify_waiters();
                    }
                }
            }

            let mut cohort = self.cohort();
            if !cohort.is_empty() {
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

                    cohort
                        .retain(|id| self.clients.get(id).map(|c| c.contributes).unwrap_or(false));
                    if cohort.is_empty() {
                        break;
                    }

                    tokio::select! {
                        _ = self.notify.notified() => {},
                        _ = tokio::time::sleep(Duration::from_millis(2)) => {},
                    }

                    while let Ok(Some(inmsg)) =
                        tokio::time::timeout(Duration::from_millis(1), self.rx.recv()).await
                    {
                        match inmsg {
                            CoreIn::FromClient(cm) => self.apply_client(cm),
                            CoreIn::AutoUnregister { app_id } => {
                                self.clients.remove(&app_id);
                                self.notify.notify_waiters();
                            }
                        }
                    }
                }

                for st in self.clients.values_mut() {
                    st.voted_tick = None;
                }
            }

            step_timer.tick().await;
            self.tick += 1;
            println!("{} step", self.tick);
            self.broadcast_state(self.tick);
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
pub struct AppLink {
    pub tx_in: mpsc::Sender<CoreIn>,
    pub rx_out: broadcast::Receiver<ServerMsg>,
}
impl AppLink {
    pub async fn send(&self, msg: ClientMsg) {
        let _ = self.tx_in.send(CoreIn::FromClient(msg)).await;
    }
    pub async fn next(&mut self) -> Option<ServerMsg> {
        self.rx_out.recv().await.ok()
    }
}

/// Spawn simulator locally (channels), optionally also expose gRPC for remote viewers/clients.
pub async fn spawn_local(
    enable_remote_service: bool,
    service_addr: Option<SocketAddr>,
) -> Result<(AppLink, tokio::task::JoinHandle<()>)> {
    use tonic::transport::Server;

    let (tx_in, rx_in) = mpsc::channel::<CoreIn>(1024);
    let (tx_out, _rx0) = broadcast::channel::<ServerMsg>(4096);

    // IMPORTANT: create the local receiver BEFORE starting the core,
    // so the local app won't miss the initial state:0 broadcast.
    let link = AppLink {
        tx_in: tx_in.clone(),
        rx_out: tx_out.subscribe(),
    };

    // Start the core
    let core = SimulatorCore::new(rx_in, tx_out.clone());
    let join = tokio::spawn(async move { core.run().await });

    // Optional gRPC service
    if enable_remote_service {
        let addr = service_addr.unwrap_or(([127, 0, 0, 1], 60000).into());
        let svc = SimulatorSvc {
            bp: Arc::new(Backplane {
                tx_in: tx_in.clone(),
                tx_out: tx_out.clone(),
            }),
        };
        tokio::spawn(async move {
            Server::builder()
                .add_service(SimulatorApiServer::new(svc))
                .serve(addr)
                .await
                .expect("gRPC service failed");
        });
        eprintln!("[local] gRPC service on {addr}");
    }

    Ok((link, join))
}

/// Standalone gRPC simulator service (for remote mode).
pub async fn run_simulator_service(listen: SocketAddr) -> Result<()> {
    let (tx_in, rx_in) = mpsc::channel::<CoreIn>(1024);
    let (tx_out, _rx0) = broadcast::channel::<ServerMsg>(4096);

    // clone for the core task so we can still use `tx_out` below
    let tx_out_for_core = tx_out.clone();
    tokio::spawn(async move {
        SimulatorCore::new(rx_in, tx_out_for_core).run().await;
    });

    let svc = SimulatorSvc {
        bp: Arc::new(Backplane { tx_in, tx_out }),
    };

    eprintln!("[remote] simulator gRPC server on {listen}");
    tonic::transport::Server::builder()
        .add_service(SimulatorApiServer::new(svc))
        .serve(listen)
        .await?;
    Ok(())
}
