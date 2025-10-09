use anyhow::Result;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant},
};

use futures_core::Stream;
use tokio::sync::{broadcast, mpsc, Notify};
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::{wrappers::BroadcastStream, StreamExt};
use tonic::{Request, Response, Status};

use sim_proto::pb::sim::{
    sim_msg::Kind,
    simulator_api_server::{SimulatorApi, SimulatorApiServer},
    Ack, SimMsg,
};

const LEASE_TIMEOUT: Duration = Duration::from_millis(5000); // heartbeat lease
const BARRIER_TIMEOUT: Duration = Duration::from_millis(5000); // max wait per tick
const ACK_TIMEOUT: Duration = Duration::from_millis(10000);

const STEP_INTERVAL: Duration = Duration::from_millis(10);
pub const CONTROL_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone)]
struct ClientStatus {
    contributes: bool,
    last_heartbeat: Instant,
    voted_tick: Option<u64>,
    last_acked_tick: u64,
    healthy: bool,
}

impl ClientStatus {
    fn new(contributes: bool) -> Self {
        Self {
            contributes,
            last_heartbeat: Instant::now(),
            voted_tick: None,
            last_acked_tick: 0,
            healthy: true,
        }
    }
}

enum Cmd {
    LegacyCommand(String), // non-fenced
    FencedCommand { app_id: String, cmd: String },
    TickSet(u64),
    Register { app_id: String, contributes: bool },
    Heartbeat { app_id: String },
    StepReady { app_id: String, tick: u64 },
    StateAck { app_id: String, tick: u64 },
    Shutdown(bool),
}

enum SimMode {
    LocalOnly,     // channels only; no external gRPC viewer
    RemoteCapable, // gRPC for viewers/remote; drain blocking cmds before step
}

/// Minimal simulator core with barrier + fences.
pub struct SimulatorCore {
    cmd_rx: mpsc::Receiver<SimMsg>,
    state_tx: broadcast::Sender<SimMsg>,
    allow_shutdown: bool,

    // barrier state
    tick: u64,
    clients: HashMap<String, ClientStatus>,
    blocking_cmds: VecDeque<(String, String)>, // (app_id, cmd)
    notify: Arc<Notify>,

    mode: SimMode,
}

impl SimulatorCore {
    pub fn new(
        cmd_rx: mpsc::Receiver<SimMsg>,
        state_tx: broadcast::Sender<SimMsg>,
        allow_shutdown: bool,
        mode: SimMode,
    ) -> Self {
        Self {
            cmd_rx,
            state_tx,
            allow_shutdown,
            tick: 0,
            clients: HashMap::new(),
            blocking_cmds: VecDeque::new(),
            notify: Arc::new(Notify::new()),
            mode,
        }
    }

    fn broadcast_state(&self, tick: u64, payload: String) {
        let _ = self.state_tx.send(SimMsg {
            kind: Some(Kind::State(payload)),
        });
        // We do not await flushing; acks are explicit via messages.
    }

    fn to_internal(msg: SimMsg) -> Option<Cmd> {
        match msg.kind {
            Some(Kind::Command(c)) => Some(Cmd::LegacyCommand(c)),
            Some(Kind::Tick(t)) => Some(Cmd::TickSet(t)),
            Some(Kind::Shutdown(b)) => Some(Cmd::Shutdown(b)),
            Some(Kind::Register(r)) => Some(Cmd::Register {
                app_id: r.app_id,
                contributes: r.contributes,
            }),
            Some(Kind::Heartbeat(hb)) => Some(Cmd::Heartbeat { app_id: hb.app_id }),
            Some(Kind::Stepready(sr)) => Some(Cmd::StepReady {
                app_id: sr.app_id,
                tick: sr.tick,
            }),
            Some(Kind::Stateack(sa)) => Some(Cmd::StateAck {
                app_id: sa.app_id,
                tick: sa.tick,
            }),
            Some(Kind::Cmd2(c2)) => {
                if c2.fence {
                    Some(Cmd::FencedCommand {
                        app_id: c2.app_id,
                        cmd: c2.cmd,
                    })
                } else {
                    // Non-fenced → do not block the barrier/free-run
                    Some(Cmd::LegacyCommand(c2.cmd))
                }
            }
            _ => None,
        }
    }

    fn maintain_health(&mut self) {
        let now = Instant::now();
        for (_id, st) in self.clients.iter_mut() {
            st.healthy = now.duration_since(st.last_heartbeat) < LEASE_TIMEOUT;
        }
    }

    fn freeze_cohort(&self) -> HashSet<String> {
        self.clients
            .iter()
            .filter(|(_, c)| c.contributes && c.healthy && c.last_acked_tick >= self.tick)
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub async fn run(mut self) {
        // Emit initial state so clients can ack tick 0
        self.broadcast_state(0, format!("state:{}", self.tick));

        let mut interval = tokio::time::interval(STEP_INTERVAL);

        loop {
            // Keep health fresh
            self.maintain_health();

            // 0) Fast-poll incoming messages into queues/state
            while let Ok(Some(msg)) =
                tokio::time::timeout(Duration::from_millis(1), self.cmd_rx.recv()).await
            {
                if let Some(cmd) = Self::to_internal(msg) {
                    self.apply_control(cmd);
                }
            }

            // 1a) If remote-capable, drain all *fenced* commands BEFORE the step
            if matches!(self.mode, SimMode::RemoteCapable) {
                while let Some((app, cmd)) = self.blocking_cmds.pop_front() {
                    //println!("[sim] applying fenced command from '{app}': {cmd}");
                    let _ = self.state_tx.send(SimMsg {
                        kind: Some(Kind::State(format!("ack:{}:{}", app, cmd))),
                    });
                }
            }

            // 1b) Barrier: wait for StepReady(t) from healthy contributors; evict laggards
            //     This runs in BOTH modes (LocalOnly and RemoteCapable).
            let mut cohort = self.freeze_cohort(); // only healthy + contributes + acked tick

            if !cohort.is_empty() {
                let barrier_deadline = Instant::now() + BARRIER_TIMEOUT;

                loop {
                    // Done if all remaining members have voted StepReady(t)
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

                    // Evict on unhealthy (heartbeat lease expired) or global barrier timeout
                    let now = Instant::now();
                    // collect evicted for logging
                    let mut evicted: Vec<String> = Vec::new();
                    cohort.retain(|id| {
                        if let Some(c) = self.clients.get(id) {
                            let keep = c.healthy && now < barrier_deadline;
                            if !keep {
                                evicted.push(id.clone());
                            }
                            keep
                        } else {
                            // unknown client id -> evict
                            evicted.push(id.clone());
                            false
                        }
                    });
                    for id in evicted {
                        eprintln!(
                            "[sim] evicting '{id}' from tick {} barrier (unhealthy or timed out)",
                            self.tick
                        );
                    }

                    if cohort.is_empty() {
                        break;
                    }

                    // While waiting, accept arriving messages and (if remote-capable) drain any newly arrived fenced cmds
                    tokio::select! {
                        _ = self.notify.notified() => {},
                        _ = tokio::time::sleep(Duration::from_millis(2)) => {},
                    }

                    while let Ok(Some(msg)) =
                        tokio::time::timeout(Duration::from_millis(1), self.cmd_rx.recv()).await
                    {
                        if let Some(cmd) = Self::to_internal(msg) {
                            self.apply_control(cmd);
                        }
                    }
                    if matches!(self.mode, SimMode::RemoteCapable) {
                        while let Some((app, cmd)) = self.blocking_cmds.pop_front() {
                            let _ = self.state_tx.send(SimMsg {
                                kind: Some(Kind::State(format!("ack:{}:{}", app, cmd))),
                            });
                        }
                    }

                    // refresh health based on latest heartbeats
                    self.maintain_health();
                }

                // Clear votes of the remaining cohort for the next tick
                for id in cohort {
                    if let Some(c) = self.clients.get_mut(&id) {
                        c.voted_tick = None;
                    }
                }
            }

            // 2) Step on the timer
            interval.tick().await;
            self.tick += 1;
            let current = self.tick;

            println!("{current} step");

            let _ = self.state_tx.send(SimMsg {
                kind: Some(Kind::State(format!("state:{}", self.tick))),
            });
        }
    }

    fn apply_control(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::FencedCommand { app_id, cmd } => {
                //println!("[sim] fenced command from '{app_id}': {cmd}");
                self.blocking_cmds.push_back((app_id, cmd));
            }
            Cmd::LegacyCommand(c) => {
                // non-fenced -> optional ack/no log
                let _ = self.state_tx.send(SimMsg {
                    kind: Some(Kind::State(format!("ack:{}", c))),
                });
            }
            Cmd::TickSet(t) => {
                self.tick = t;
                self.broadcast_state(self.tick, format!("retick:{}", self.tick));
            }
            Cmd::Register {
                app_id,
                contributes,
            } => {
                let st = self
                    .clients
                    .entry(app_id)
                    .or_insert_with(|| ClientStatus::new(contributes));
                st.contributes = contributes;
                st.last_heartbeat = Instant::now();
                st.healthy = true;
            }
            Cmd::Heartbeat { app_id } => {
                if let Some(st) = self.clients.get_mut(&app_id) {
                    st.last_heartbeat = Instant::now();
                    st.healthy = true;
                } else {
                    // late heartbeat without register: create non-contributing entry
                    self.clients.insert(app_id, ClientStatus::new(false));
                }
            }
            Cmd::StepReady { app_id, tick } => {
                if let Some(st) = self.clients.get_mut(&app_id) {
                    // accept idempotently only for current tick
                    if tick == self.tick {
                        st.voted_tick = Some(tick);
                    }
                }
            }
            Cmd::StateAck { app_id, tick } => {
                if let Some(st) = self.clients.get_mut(&app_id) {
                    if tick > st.last_acked_tick {
                        st.last_acked_tick = tick;
                    }
                }
            }
            Cmd::Shutdown(flag) => {
                if self.allow_shutdown && flag {
                    let _ = self.state_tx.send(SimMsg {
                        kind: Some(Kind::State("shutdown".into())),
                    });
                    // graceful stop is handled by process termination in this design
                } else {
                    let _ = self.state_tx.send(SimMsg {
                        kind: Some(Kind::State("shutdown_ignored".into())),
                    });
                }
            }
        }
        self.notify.notify_waiters();
    }
}

#[derive(Clone)]
pub struct Backplane {
    pub cmd_tx: mpsc::Sender<SimMsg>,
    pub state_tx: broadcast::Sender<SimMsg>,
}

#[derive(Clone)]
pub struct SimulatorSvc {
    bp: Arc<Backplane>,
}

#[tonic::async_trait]
impl SimulatorApi for SimulatorSvc {
    async fn send(&self, req: Request<SimMsg>) -> Result<Response<Ack>, Status> {
        let msg = req.into_inner();
        self.bp.cmd_tx.try_send(msg).map_err(|e| match e {
            tokio::sync::mpsc::error::TrySendError::Full(_) => {
                Status::resource_exhausted("command buffer full")
            }
            _ => Status::unavailable("simulator not available"),
        })?;
        Ok(Response::new(Ack { ok: true }))
    }

    type SubscribeStream = Pin<Box<dyn Stream<Item = Result<SimMsg, Status>> + Send + 'static>>;

    async fn subscribe(
        &self,
        _req: tonic::Request<sim_proto::pb::sim::Empty>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        let rx = self.bp.state_tx.subscribe();
        let stream = BroadcastStream::new(rx).map(|res| match res {
            Ok(msg) => Ok(msg),
            Err(BroadcastStreamRecvError::Lagged(_)) => Ok(SimMsg {
                kind: Some(Kind::State("lagged".into())),
            }),
        });
        Ok(Response::new(Box::pin(stream)))
    }
}

/// Local app link (channels).
pub struct AppLink {
    pub cmd_tx: mpsc::Sender<SimMsg>,
    pub state_rx: broadcast::Receiver<SimMsg>,
}
impl AppLink {
    pub async fn send(&self, msg: SimMsg) {
        let _ = self.cmd_tx.send(msg).await;
    }
    pub async fn next_state(&mut self) -> Option<SimMsg> {
        self.state_rx.recv().await.ok()
    }
}

/// Spawn simulator locally (channels), optionally also expose gRPC for remote clients/viewers.
pub async fn spawn_local(
    enable_remote_client: bool,
    rc_addr: Option<SocketAddr>,
) -> Result<(AppLink, tokio::task::JoinHandle<()>)> {
    let (cmd_tx, cmd_rx) = mpsc::channel::<SimMsg>(1024);
    let (state_tx, _rx0) = broadcast::channel::<SimMsg>(4096);

    let mode = if enable_remote_client {
        SimMode::RemoteCapable
    } else {
        SimMode::LocalOnly
    };

    // Optional remote-client gRPC server
    if enable_remote_client {
        let addr = rc_addr.unwrap_or(([127, 0, 0, 1], 60000).into());
        let svc = SimulatorSvc {
            bp: Arc::new(Backplane {
                cmd_tx: cmd_tx.clone(),
                state_tx: state_tx.clone(),
            }),
        };
        tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(SimulatorApiServer::new(svc))
                .serve(addr)
                .await
                .expect("remote-client server failed");
        });
        eprintln!("[local] remote-client gRPC server on {addr}");
    }

    let core = SimulatorCore::new(cmd_rx, state_tx.clone(), true, mode);
    let join = tokio::spawn(async move { core.run().await });

    let link = AppLink {
        cmd_tx,
        state_rx: state_tx.subscribe(),
    };
    Ok((link, join))
}

/// Standalone gRPC simulator service (for remote mode).
pub async fn run_simulator_service(listen: SocketAddr) -> Result<()> {
    let (cmd_tx, cmd_rx) = mpsc::channel::<SimMsg>(1024);
    let (state_tx, _rx0) = broadcast::channel::<SimMsg>(4096);

    let mode = SimMode::RemoteCapable; // remote service always remote-capable

    // Remote service: full barrier always (freerun=false)
    let state_tx_core = state_tx.clone();

    tokio::spawn(async move {
        SimulatorCore::new(cmd_rx, state_tx_core, false, mode)
            .run()
            .await;
    });

    let svc = SimulatorSvc {
        bp: Arc::new(Backplane { cmd_tx, state_tx }),
    };
    eprintln!("[remote] simulator gRPC server on {listen}");
    tonic::transport::Server::builder()
        .add_service(SimulatorApiServer::new(svc))
        .serve(listen)
        .await?;
    Ok(())
}
