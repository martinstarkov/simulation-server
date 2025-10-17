use anyhow::Result;
use crossbeam_channel as xchan;
use futures::StreamExt;
use interface::{
    ClientMsg, ErrorMsg, ServerMode, ServerMsg, ServerMsgBody, Simulation, Tick,
    forwarder_server::{Forwarder, ForwarderServer},
};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::info;

pub(crate) enum ServerRequest {
    Register { contributing: bool },
    Unregister { client_id: u64 },
    StepReady { client_id: u64 },
    Shutdown,
}

pub(crate) enum ServerResponse {
    Registered {
        client_id: u64,
        rx_sv_msg: xchan::Receiver<ServerMsg>,
        rx_sv_resp: xchan::Receiver<ServerResponse>,
    },
    Unregistered,
    Stepped {
        tick: Tick,
    },
    Shutdown,
}

#[derive(Debug)]
pub struct Server<S: Simulation> {
    pub tx_sv_resp: xchan::Sender<ServerResponse>,
    pub rx_sv_resp: xchan::Receiver<ServerResponse>,

    pub tx_sv_req: xchan::Sender<ServerRequest>,
    pub rx_sv_req: xchan::Receiver<ServerRequest>,

    pub tx_sv_msg: xchan::Sender<ServerMsg>,
    pub rx_sv_msg: xchan::Receiver<ServerMsg>,

    pub tx_cl_msg: xchan::Sender<ClientMsg>,
    pub rx_cl_msg: xchan::Receiver<ClientMsg>,

    pub sim: Arc<Mutex<S>>,
}

impl<S: Simulation> Server<S> {
    fn spawn_service(
        tx_cl_msg: xchan::Sender<ClientMsg>,
        rx_sv_msg: xchan::Receiver<ServerMsg>,
        tx_sv_req: xchan::Sender<ServerRequest>,
    ) -> JoinHandle<Result<()>> {
        todo!();
        // thread::spawn(move || {
        //     safe_block_on(async move {
        //         info!("[Server] Starting with gRPC on {addr}");
        //         let service = ForwarderService {
        //             tx_cl_msg,
        //             rx_sv_msg,
        //             tx_sv_req,
        //         };
        //         tonic::transport::Server::builder()
        //             .add_service(ForwarderServer::new(service))
        //             .serve(addr.parse().unwrap())
        //             .await
        //             .unwrap();
        //     });
        // });
    }

    fn spawn_server(
        rx_sv_req: xchan::Receiver<ServerRequest>,
        rx_cl_msg: xchan::Receiver<ClientMsg>,
        tx_sv_resp: xchan::Sender<ServerResponse>,
        tx_sv_msg: xchan::Sender<ServerMsg>,
        sim: Arc<Mutex<S>>,
    ) {
        thread::spawn(move || {
            struct ClientChannels {
                pub msg: xchan::Sender<ServerMsg>,
                pub resp: xchan::Sender<ServerResponse>,
            }

            #[derive(Default)]
            struct State {
                next_id: u64,
                contributing: HashSet<u64>,
                ready: HashSet<u64>,
                client_out: HashMap<u64, ClientChannels>,
                tick_seq: u64,
            }

            let mut st = State::default();

            let dt = sim.lock().unwrap().dt();

            loop {
                // Use select! to listen to both message sources
                xchan::select! {
                    recv(rx_sv_req) -> req => {
                        if let Ok(req) = req {
                            match req {
                                ServerRequest::Register { contributing } => {
                                    st.next_id += 1;
                                    let id = st.next_id;
                                    let (out_tx, out_rx) = xchan::bounded::<ServerMsg>(256);
                                    let (out_resp_tx, out_resp_rx) = xchan::bounded::<ServerResponse>(256);
                                    let c = ClientChannels{
                                        msg: out_tx.clone(),
                                        resp: out_resp_tx.clone()
                                    };
                                    st.client_out.insert(id, c);
                                    if contributing {
                                        st.contributing.insert(id);
                                    }

                                    info!("[Server] Registered client #{id} (contrib={})", contributing);

                                    // Send back registration info
                                    let resp = ServerResponse::Registered { client_id: id, rx_sv_msg: out_rx, rx_sv_resp: out_resp_rx };
                                    let _ = tx_sv_resp.send(resp);
                                }

                                ServerRequest::Unregister { client_id } => {
                                    info!("[Server] Unregistered client #{client_id}");
                                    st.contributing.remove(&client_id);
                                    st.ready.remove(&client_id);
                                    st.client_out.remove(&client_id);
                                }

                                ServerRequest::Shutdown => {
                                    info!("[Server] Shutdown requested");
                                    break;
                                }

                                ServerRequest::StepReady { client_id } => {
                                    if !st.contributing.contains(&client_id) {
                                        continue;
                                    }
                                    st.ready.insert(client_id);

                                    // Barrier sync
                                    if st.ready.len() == st.contributing.len() {
                                        st.tick_seq += 1;
                                        let tick = Tick {
                                            seq: st.tick_seq,
                                            time_s: (st.tick_seq as f32) * dt,
                                        };
                                        info!("[Server] Stepped to tick {}", st.tick_seq);

                                        for c in st.client_out.values() {
                                            c.msg.send(ServerMsg {
                                                 body: Some(ServerMsgBody::Tick(tick.clone()))
                                             });
                                             c.resp.send(ServerResponse::Stepped { tick: tick.clone() });
                                        }

                                        st.ready.clear();
                                    }
                                }
                            }
                        } else {
                            info!("[Server] ServerRequest channel closed");
                            // channel closed
                            break;
                        }
                    },

                    recv(rx_cl_msg) -> msg => {
                        if let Ok(msg) = msg {
                            // Forward client messages into simulation or coordinator
                            // This depends on your protocol – here we just broadcast
                            info!("[Server] Received ClientMsg: {:?}", msg);

                            // pass to simulation
                            // TODO: Get rid of ok().unwrap().
                            let responses = sim.lock().unwrap().handle_message(msg).ok().unwrap();
                            for msg in responses {
                                for c in st.client_out.values() {
                                    let _ = c.msg.send(msg.clone());
                                }
                            }
                        } else {
                            info!("[Server] All client channels closed");
                            // All client channels closed
                            break;
                        }
                    },
                }
            }
        });
    }

    pub fn new_with_grpc(addr: &str, simulator: S) -> Arc<Self> {
        todo!();
        //spawn_server();
        // spawn_service(tx_cl_msg.clone(), rx_sv_msg.clone(), tx_sv_req.clone());
    }

    pub fn new_without_grpc(simulator: S) -> Arc<Self> {
        let (tx_sv_req, rx_sv_req) = xchan::unbounded::<ServerRequest>();
        let (tx_sv_resp, rx_sv_resp) = xchan::unbounded::<ServerResponse>();
        let (tx_sv_msg, rx_sv_msg) = xchan::unbounded::<ServerMsg>();
        let (tx_cl_msg, rx_cl_msg) = xchan::unbounded::<ClientMsg>();

        let sim = Arc::new(Mutex::new(simulator));

        Self::spawn_server(
            rx_sv_req.clone(),
            rx_cl_msg.clone(),
            tx_sv_resp.clone(),
            tx_sv_msg.clone(),
            Arc::clone(&sim),
        );

        Arc::new(Self {
            tx_sv_resp,
            rx_sv_resp,
            tx_sv_req,
            rx_sv_req,
            tx_sv_msg,
            rx_sv_msg,
            tx_cl_msg,
            rx_cl_msg,
            sim,
        })
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

/* ===================== Tonic gRPC service ===================== */

#[derive(Clone)]
pub struct ForwarderService {
    pub tx_cl_msg: xchan::Sender<ClientMsg>,
    pub rx_sv_msg: xchan::Receiver<ServerMsg>,
    pub tx_sv_req: xchan::Sender<ServerRequest>,
}

#[tonic::async_trait]
impl Forwarder for ForwarderService {
    type OpenStream = ReceiverStream<Result<ServerMsg, Status>>;

    async fn open(
        &self,
        req: Request<tonic::Streaming<ClientMsg>>,
    ) -> Result<Response<Self::OpenStream>, Status> {
        let mut inbound = req.into_inner();

        // mpsc channel → this is what tonic uses to send responses back to the client
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<ServerMsg, Status>>(32);

        // Clone handles to crossbeam channels
        let tx_cl_msg = self.tx_cl_msg.clone();
        let rx_sv_msg = self.rx_sv_msg.clone();
        let tx_sv_req = self.tx_sv_req.clone();

        // Spawn async task to forward messages in both directions
        tokio::spawn(async move {
            // Spawn separate task to pump ServerMsg from app → gRPC stream
            let tx_clone = tx.clone();
            let rx_sv_msg_clone = rx_sv_msg.clone();
            tokio::spawn(async move {
                while let Ok(msg) = rx_sv_msg_clone.recv() {
                    if tx_clone.send(Ok(msg)).await.is_err() {
                        break; // client hung up
                    }
                }
            });

            let mut client_id = None;

            // Handle inbound ClientMsg from gRPC → app
            while let Some(Ok(msg)) = inbound.next().await {
                client_id = Some(msg.client_id);
                if tx_cl_msg.send(msg).is_err() {
                    break; // app side closed
                }
            }

            if let Some(client_id) = client_id {
                info!("[Coordinator] Client #{client_id} disconnected");
                if tx_sv_req
                    .send(ServerRequest::Unregister { client_id })
                    .is_err()
                {
                    info!("[Coordinator] Failed to send Unregister for client #{client_id}");
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}
