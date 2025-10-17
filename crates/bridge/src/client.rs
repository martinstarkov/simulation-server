use futures::StreamExt;
use std::{
    sync::Arc,
    thread::{self},
    time::Duration,
};

use anyhow::{Result, anyhow};
use crossbeam_channel::{self as xchan};

use interface::{
    ClientMsg, ErrorMsg, ServerMsg, ServerMsgBody, Simulation, Tick,
    forwarder_client::ForwarderClient,
};

use tokio_stream::wrappers::ReceiverStream;
use tracing::{error, info};

use crate::server::{Server, ServerRequest, ServerResponse, safe_block_on};

#[derive(Clone)]
pub struct Client {
    contributing: bool,
    id: u64,
    tx_sv_req: xchan::Sender<ServerRequest>,
    rx_sv_resp: xchan::Receiver<ServerResponse>,
    tx_cl_msg: xchan::Sender<ClientMsg>,
    rx_sv_msg: xchan::Receiver<ServerMsg>,
}

impl Client {
    pub fn new_local<S: Simulation>(contributing: bool, sv: &Arc<Server<S>>) -> Result<Self> {
        let server = &Arc::clone(&sv);

        let tx_sv_req = server.tx_sv_req.clone();
        let rx_sv_resp = server.rx_sv_resp.clone();

        tx_sv_req.send(ServerRequest::Register { contributing })?;

        let (client_id, rx_sv_msg, rx_sv_resp) = loop {
            match rx_sv_resp.recv()? {
                ServerResponse::Registered {
                    client_id,
                    rx_sv_msg,
                    rx_sv_resp,
                } => break (client_id, rx_sv_msg, rx_sv_resp),
                _ => {} // wait until we see Registered.
            }
        };

        info!("[Client #{client_id}] Registered with the server");

        Ok(Self {
            contributing,
            id: client_id,
            tx_sv_req,
            rx_sv_resp,
            tx_cl_msg: server.tx_cl_msg.clone(),
            rx_sv_msg: rx_sv_msg,
        })
    }

    /// Connect to a **remote** gRPC server at the given address.
    ///
    /// You can pass either:
    /// - `"127.0.0.1:50051"` → automatically becomes `"http://127.0.0.1:50051"`
    /// - `"http://127.0.0.1:50051"` → used as is
    /// Connect to a **remote** gRPC server at the given address
    pub fn new_remote(contributing: bool, addr: &str) -> Result<Self> {
        let (tx_sv_req, rx_sv_req) = xchan::bounded::<ServerRequest>(1024);
        let (tx_sv_resp, rx_sv_resp) = xchan::bounded::<ServerResponse>(1024);
        let (tx_cl_msg, rx_cl_msg) = xchan::bounded::<ClientMsg>(1024);
        let (tx_sv_msg, rx_sv_msg) = xchan::bounded::<ServerMsg>(1024);

        let addr_http = if addr.starts_with("http://") || addr.starts_with("https://") {
            addr.to_string()
        } else {
            format!("http://{}", addr)
        };

        info!("[Client] Connecting to {addr_http}");

        // Spawn a background thread to handle the gRPC streaming
        thread::spawn({
            let addr_http = addr_http.clone();
            let rx_cl_msg = rx_cl_msg.clone();
            let tx_sv_msg = tx_sv_msg.clone();

            move || {
                safe_block_on(async move {
                    // 1. Connect to the gRPC server
                    let mut grpc = match ForwarderClient::connect(addr_http.clone()).await {
                        Ok(c) => {
                            info!("[Client] Connected to gRPC on {addr_http}");
                            c
                        }
                        Err(e) => {
                            error!("[Client] Failed to connect: {e}");
                            let _ = tx_sv_msg.send(ServerMsg {
                                body: Some(ServerMsgBody::ErrorMsg(ErrorMsg {
                                    message: format!("connect error: {e}"),
                                })),
                            });
                            return;
                        }
                    };

                    // 2. Create outbound stream (ClientMsg)
                    let (tx_out, rx_out) = tokio::sync::mpsc::channel::<ClientMsg>(1024);
                    let rx_stream = ReceiverStream::new(rx_out);

                    // 3. Open bidirectional gRPC stream
                    let mut stream = match grpc.open(rx_stream).await {
                        Ok(resp) => resp.into_inner(),
                        Err(e) => {
                            let _ = tx_sv_msg.send(ServerMsg {
                                body: Some(ServerMsgBody::ErrorMsg(ErrorMsg {
                                    message: format!("stream open error: {e}"),
                                })),
                            });
                            return;
                        }
                    };

                    // 4. Inbound: gRPC → app channel
                    let tx_sv_msg_clone = tx_sv_msg.clone();
                    tokio::spawn(async move {
                        while let Some(Ok(msg)) = stream.next().await {
                            let _ = tx_sv_msg_clone.send(msg);
                        }
                    });

                    // 5. Outbound: app → gRPC stream
                    while let Ok(msg) = rx_cl_msg.recv() {
                        if tx_out.blocking_send(msg).is_err() {
                            break;
                        }
                    }
                });
            }
        });

        tx_sv_req.send(ServerRequest::Register { contributing })?;

        let (client_id, rx_sv_msg, rx_sv_resp) = loop {
            match rx_sv_resp.recv()? {
                ServerResponse::Registered {
                    client_id,
                    rx_sv_msg,
                    rx_sv_resp,
                } => break (client_id, rx_sv_msg, rx_sv_resp),
                _ => {} // wait until we see Registered.
            }
        };

        info!("[Client #{client_id}] Registered with the server");

        Ok(Self {
            contributing,
            id: client_id,
            tx_sv_req,
            rx_sv_resp,
            tx_cl_msg,
            rx_sv_msg,
        })
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn contributing(&self) -> bool {
        self.contributing
    }

    /// Send an outbound `ClientMsg` to the simulator (blocking).
    pub fn send(&self, msg: ClientMsg) -> Result<()> {
        self.tx_cl_msg
            .send(msg)
            .map_err(|e| anyhow!("send error: {e}"))
    }

    /// Receive the next inbound `ServerMsg` (blocking).
    pub fn recv(&self) -> Result<ServerMsg> {
        self.rx_sv_msg
            .recv()
            .map_err(|e| anyhow!("recv error: {e}"))
    }

    /// Non-blocking receive; returns `None` if no message is ready.
    pub fn try_recv(&self) -> Option<ServerMsg> {
        self.rx_sv_msg.try_recv().ok()
    }

    pub fn step_ready(&self) -> Result<Tick> {
        info!("[Client #{}] Waiting for tick from server...", self.id);

        self.tx_sv_req
            .send(ServerRequest::StepReady { client_id: self.id })
            .map_err(|e| anyhow!("send error: {e}"))?;

        Ok(loop {
            match self.rx_sv_resp.recv()? {
                ServerResponse::Stepped { tick } => break tick,
                _ => {} // wait until we see Registered.
            }
        })
    }

    /// Forward all inbound `ServerMsg` to the tx channel.
    pub fn forward_all(&self, tx: &xchan::Sender<ServerMsg>) -> Result<()> {
        loop {
            match self.rx_sv_msg.try_recv() {
                Ok(msg) => tx.send(msg)?,
                Err(_) => break,
            }
        }
        Ok(())
    }
}

fn spawn_step_thread(work_ms: u64, sim: Client) -> thread::JoinHandle<Result<()>> {
    thread::spawn(move || {
        loop {
            sim.step_ready()?;
            std::thread::sleep(Duration::from_millis(work_ms));
        }
    })
}

/// Connect a thread which just calls StepReady on the remote server, acting as a contributing client with no "work".
pub fn connect_remote_stepper(addr: &str, work_ms: u64) -> thread::JoinHandle<Result<()>> {
    let remote_client = Client::new_remote(true, &addr);

    spawn_step_thread(work_ms, remote_client.expect("connect to remote failed"))
}
