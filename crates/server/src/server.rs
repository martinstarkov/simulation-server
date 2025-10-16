use crossbeam_channel as xchan;
use interface::{
    ServerMode, Simulation,
    interface::{
        ClientMsg, ServerMsg, Tick, server_msg,
        simulator_server::{Simulator, SimulatorServer},
    },
};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    thread,
};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::info;

#[derive(Clone, Debug)]
pub struct SimServer<S: Simulation> {
    pub coord: Coordinator<S>,
}

impl<S: Simulation> SimServer<S> {
    pub fn new(sim: Arc<Mutex<S>>) -> Self {
        let coord = Coordinator::spawn(sim);
        Self { coord }
    }
}

/// Run an async future safely, reusing an existing Tokio runtime if one exists.
///
/// - If already inside a Tokio runtime: executes the future synchronously.
/// - If no runtime exists: creates a lightweight single-threaded runtime.
pub fn safe_block_on<F: std::future::Future>(fut: F) -> F::Output {
    if tokio::runtime::Handle::try_current().is_ok() {
        // Already in a runtime: block in place
        tokio::task::block_in_place(|| futures::executor::block_on(fut))
    } else {
        // Create a lightweight current-thread runtime
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        rt.block_on(fut)
    }
}

pub fn create_custom_server<S: Simulation>(mode: ServerMode, sim: Arc<Mutex<S>>) -> SimServer<S> {
    let server = SimServer::new(sim.clone());

    match mode {
        ServerMode::LocalOnly => server,
        ServerMode::WithGrpc(addr) => {
            let coord = server.coord.clone();

            let _join = thread::Builder::new()
                .name("grpc-server".into())
                .spawn(move || {
                    safe_block_on(async move {
                        info!("starting gRPC server on {addr}");
                        let svc = SimSvc { coord };
                        tonic::transport::Server::builder()
                            .add_service(SimulatorServer::new(svc))
                            .serve(addr.parse().unwrap())
                            .await
                            .unwrap();
                    });
                });

            server
        }
    }
}

#[derive(Debug)]
pub struct Coordinator<S: Simulation> {
    tx: xchan::Sender<Cmd<S>>,
}

impl<S: Simulation> Clone for Coordinator<S> {
    fn clone(&self) -> Self {
        Coordinator {
            tx: self.tx.clone(),
        }
    }
}

enum Cmd<S: Simulation> {
    Register {
        name: String,
        contributing: bool,
        reply: xchan::Sender<(u64, xchan::Receiver<ServerMsg>)>,
    },
    StepReady {
        client_id: u64,
    },
    Remove {
        client_id: u64,
    },
    Message {
        msg: ClientMsg,
    },
    Shutdown,
    _Phantom(std::marker::PhantomData<S>),
}

impl<S: Simulation> Coordinator<S> {
    pub fn spawn(sim: Arc<Mutex<S>>) -> Coordinator<S> {
        let (tx, rx) = xchan::unbounded::<Cmd<S>>();

        std::thread::spawn(move || {
            #[derive(Default)]
            struct State {
                next_id: u64,
                contributing: HashSet<u64>,
                ready: HashSet<u64>,
                client_out: HashMap<u64, xchan::Sender<ServerMsg>>,
                tick_seq: u64,
            }

            let mut st = State::default();
            while let Ok(cmd) = rx.recv() {
                match cmd {
                    Cmd::Register {
                        name: _,
                        contributing,
                        reply,
                        ..
                    } => {
                        st.next_id += 1;
                        let id = st.next_id;
                        let (out_tx, out_rx) = xchan::bounded::<ServerMsg>(256);
                        st.client_out.insert(id, out_tx.clone());
                        if contributing {
                            st.contributing.insert(id);
                        }
                        info!("Server registered client #{id} (contrib={})", contributing);
                        let _ = reply.send((id, out_rx));
                    }
                    Cmd::StepReady { client_id } => {
                        if !st.contributing.contains(&client_id) {
                            continue;
                        }
                        st.ready.insert(client_id);
                        if st.ready.len() == st.contributing.len() {
                            // barrier met
                            st.tick_seq += 1;
                            let tick = Tick {
                                seq: st.tick_seq,
                                time_ns: 0,
                            };
                            info!("Server stepped forward to tick: {}", st.tick_seq);
                            for tx in st.client_out.values() {
                                let _ = tx.send(ServerMsg {
                                    msg: Some(server_msg::Msg::Tick(tick.clone())),
                                });
                            }
                            st.ready.clear();
                        }
                    }
                    Cmd::Message { msg, .. } => {
                        // pass to simulation
                        let responses = sim.lock().unwrap().handle_message(msg);
                        match responses {
                            Ok(resps) => {
                                for r in resps {
                                    // broadcast (can change to directed send later)
                                    for tx in st.client_out.values() {
                                        let _ = tx.send(r.clone());
                                    }
                                }
                            }
                            Err(e) => {
                                for tx in st.client_out.values() {
                                    let _ = tx.send(ServerMsg {
                                        msg: Some(server_msg::Msg::ErrorMsg(
                                            interface::interface::ErrorMsg {
                                                message: format!("sim error: {e}"),
                                            },
                                        )),
                                    });
                                }
                            }
                        }
                    }
                    Cmd::Remove { client_id } => {
                        info!("Server unregistered client #{client_id}");
                        st.contributing.remove(&client_id);
                        st.ready.remove(&client_id);
                        st.client_out.remove(&client_id);
                    }
                    Cmd::Shutdown => {
                        info!("Server shutdown");
                        break;
                    }
                    Cmd::_Phantom(_) => {
                        panic!("Phantom server command");
                    }
                }
            }
        });

        Coordinator { tx }
    }

    pub fn register(&self, name: &str, contributing: bool) -> (u64, xchan::Receiver<ServerMsg>) {
        let (reply_tx, reply_rx) = xchan::bounded::<(u64, xchan::Receiver<ServerMsg>)>(1);
        let _ = self.tx.send(Cmd::Register {
            name: name.into(),
            contributing,
            reply: reply_tx,
        });
        reply_rx.recv().expect("coordinator down")
    }

    pub fn step_ready(&self, client_id: u64) {
        let _ = self.tx.send(Cmd::StepReady { client_id });
    }

    pub fn send_message(&self, msg: ClientMsg) {
        let _ = self.tx.send(Cmd::Message { msg });
    }

    pub fn remove(&self, client_id: u64) {
        let _ = self.tx.send(Cmd::Remove { client_id });
    }
}

/* ===================== Tonic gRPC service ===================== */

#[derive(Clone)]
struct SimSvc<S: Simulation> {
    coord: Coordinator<S>,
}

#[tonic::async_trait]
impl<S: Simulation> Simulator for SimSvc<S> {
    type OpenStream = ReceiverStream<Result<ServerMsg, Status>>;

    async fn open(
        &self,
        req: Request<tonic::Streaming<ClientMsg>>,
    ) -> Result<Response<Self::OpenStream>, Status> {
        let mut inbound = req.into_inner();
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<ServerMsg, Status>>(32);
        let coord = self.coord.clone();

        // Drive inbound messages in an async task
        tokio::spawn(async move {
            let mut client_id: Option<u64> = None;

            while let Ok(Some(msg)) = inbound.message().await {
                match msg.msg {
                    // Contributing clients tick barrier
                    Some(interface::interface::client_msg::Msg::StepReady(sr)) => {
                        coord.step_ready(sr.client_id);
                    }

                    // Registration: reply with Registered immediately, then forward coordinator stream
                    Some(interface::interface::client_msg::Msg::Register(req)) => {
                        let (id, out_rx) = coord.register(&req.client_name, req.contributing);
                        client_id = Some(id);

                        // 1) Send Registered immediately so the client can proceed
                        let _ = tx
                            .send(Ok(ServerMsg {
                                msg: Some(interface::interface::server_msg::Msg::Registered(
                                    interface::interface::RegisterResponse { client_id: id },
                                )),
                            }))
                            .await;

                        // 2) Forward coordinator -> stream on a blocking thread
                        //    (crossbeam receiver is blocking; use `blocking_send` on tokio mpsc)
                        let tx_clone = tx.clone();
                        std::thread::spawn(move || {
                            for m in out_rx.iter() {
                                // best-effort; if the stream is closed, we just stop
                                let _ = tx_clone.blocking_send(Ok(m));
                            }
                        });
                    }

                    // Everything else goes to the simulation handler through the coordinator
                    _ => coord.send_message(msg),
                }
            }

            if let Some(id) = client_id {
                info!("[Coordinator] Client #{id} disconnected");
                coord.remove(id);
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn get_latest_tick(&self, _req: Request<()>) -> Result<Response<Tick>, Status> {
        Ok(Response::new(Tick { seq: 0, time_ns: 0 }))
    }

    async fn get_latest_state(
        &self,
        _req: Request<()>,
    ) -> Result<Response<interface::interface::Observation>, Status> {
        Err(Status::unimplemented("no state API"))
    }
}
