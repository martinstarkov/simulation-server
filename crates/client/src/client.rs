use std::{
    sync::Arc,
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::{Result, anyhow};
use crossbeam_channel::{self as xchan};

use interface::{
    ClientMsg, ClientMsgBody, ErrorMsg, RegisterRequest, RegisterResponse, ServerMsg,
    ServerMsgBody, Simulation, StepReady, Tick, simulator_client::SimulatorClient,
};

use server::server::{SimServer, safe_block_on};
use tracing::{error, info};

// ===== Public sync facade =====================================================

/// Transport-agnostic, synchronous client handle.
pub struct SyncSimClient {
    contributing: bool,
    tx_to_worker: xchan::Sender<ClientMsg>,
    rx_from_worker: xchan::Receiver<ServerMsg>,
    id: u64,
    _join: JoinHandle<()>,
}

impl SyncSimClient {
    pub fn try_new(
        contributing: bool,
        tx_to_worker: xchan::Sender<ClientMsg>,
        rx_from_worker: xchan::Receiver<ServerMsg>,
        join: JoinHandle<()>,
    ) -> Result<Self> {
        let mut client = Self {
            contributing,
            tx_to_worker,
            rx_from_worker,
            id: 0,
            _join: join,
        };

        client.id = client.register(contributing)?;

        Ok(client)
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn contributing(&self) -> bool {
        self.contributing
    }

    /// Send an outbound `ClientMsg` to the simulator (blocking on channel backpressure).
    pub fn send(&self, msg: ClientMsg) -> Result<()> {
        self.tx_to_worker
            .send(msg)
            .map_err(|e| anyhow!("sync_client send error: {e}"))
    }

    /// Receive the next inbound `ServerMsg` (blocking).
    pub fn recv(&self) -> Result<ServerMsg> {
        self.rx_from_worker
            .recv()
            .map_err(|e| anyhow!("sync_client recv error: {e}"))
    }

    /// Non-blocking receive; returns `None` if no message is ready.
    pub fn try_recv(&self) -> Option<ServerMsg> {
        self.rx_from_worker.try_recv().ok()
    }

    pub fn step_ready(&self) -> Result<()> {
        info!("[Client #{}] Waiting for tick from server...", self.id);
        self.send(ClientMsg {
            client_id: self.id,
            body: Some(ClientMsgBody::StepReady(StepReady {})),
        })
    }

    pub fn forward_until_tick(&self, tx_app: &xchan::Sender<ServerMsg>) -> Result<()> {
        loop {
            let msg = self.recv()?; // get next message
            let is_tick = matches!(msg.body.as_ref(), Some(ServerMsgBody::Tick(_)));

            tx_app.send(msg)?; // forward it (including tick)

            if is_tick {
                break; // stop after forwarding the tick
            }
        }
        Ok(())
    }

    pub fn wait_for_tick(&self) -> Result<Tick> {
        loop {
            match self.recv()? {
                ServerMsg {
                    body: Some(ServerMsgBody::Tick(tick @ Tick { .. })),
                    ..
                } => {
                    return Ok(tick);
                }
                _ => {}
            }
        }
    }

    /// Register the client (blocking).
    fn register(&mut self, contributing: bool) -> Result<u64> {
        self.send_register(contributing)?;
        let client_id = self.wait_until_registered();
        anyhow::Ok(client_id)?
    }

    fn send_register(&mut self, contributing: bool) -> Result<()> {
        self.contributing = contributing;
        self.send(ClientMsg {
            client_id: 0, /* No id known yet */
            body: Some(ClientMsgBody::Register(RegisterRequest { contributing })),
        })
    }

    /// @return Client id.
    fn wait_until_registered(&self) -> Result<u64> {
        let client_id = loop {
            match self.recv()? {
                ServerMsg {
                    body: Some(ServerMsgBody::Registered(RegisterResponse { client_id })),
                } => break client_id,
                _ => {} // ignore until we see Registered.
            }
        };
        info!("[Client #{client_id}] Registered with the server and received RegisterResponse");
        Ok(client_id)
    }
}

/// Connect to a **remote** gRPC server at the given address.
///
/// You can pass either:
/// - `"127.0.0.1:50051"` → automatically becomes `"http://127.0.0.1:50051"`
/// - `"http://127.0.0.1:50051"` → used as is
/// Connect to a **remote** gRPC server at the given address.
pub fn connect_remote(contributing: bool, addr: &str) -> Result<SyncSimClient> {
    let (to_worker_tx, to_worker_rx) = xchan::bounded::<ClientMsg>(1024);
    let (to_app_tx, to_app_rx) = xchan::bounded::<ServerMsg>(1024);

    // Automatically prepend "http://" if not present
    let addr_http = if addr.starts_with("http://") || addr.starts_with("https://") {
        addr.to_string()
    } else {
        format!("http://{}", addr)
    };

    info!("[Client] Connecting to {addr_http}");

    let join = std::thread::spawn(move || {
        // Each worker thread runs its own lightweight single-threaded runtime
        safe_block_on(async move {
            // --- 1) Connect to gRPC server ---
            let mut grpc = match SimulatorClient::connect(addr_http.clone()).await {
                Ok(c) => {
                    info!("[Client] Connected to gRPC on {addr_http}");
                    c
                }
                Err(e) => {
                    error!("[Client] Failed to connect: {e}");
                    let _ = to_app_tx.send(ServerMsg {
                        body: Some(ServerMsgBody::ErrorMsg(ErrorMsg {
                            message: format!("connect error: {e}"),
                        })),
                    });
                    return;
                }
            };

            // --- 2) Outbound stream (ClientMsg) ---
            let (out_tx, out_rx) = tokio::sync::mpsc::channel::<ClientMsg>(1024);

            // --- 3) Open bidirectional stream ---
            let mut srv_stream = match grpc
                .open(tokio_stream::wrappers::ReceiverStream::new(out_rx))
                .await
            {
                Ok(resp) => resp.into_inner(),
                Err(e) => {
                    let _ = to_app_tx.send(ServerMsg {
                        body: Some(ServerMsgBody::ErrorMsg(ErrorMsg {
                            message: format!("open stream error: {e}"),
                        })),
                    });
                    return;
                }
            };

            // --- 4) Pump inbound ServerMsg → app channel (async) ---
            let app_tx = to_app_tx.clone();
            tokio::spawn(async move {
                while let Ok(Some(msg)) = srv_stream.message().await {
                    let _ = app_tx.send(msg);
                }
                // stream ended; nothing else to do
            });

            // --- 5) Pump outbound App ClientMsg → gRPC stream (blocking thread) ---
            let out_tx_clone = out_tx.clone();
            let to_worker_rx_clone = to_worker_rx.clone();
            std::thread::spawn(move || {
                while let Ok(m) = to_worker_rx_clone.recv() {
                    if out_tx_clone.blocking_send(m).is_err() {
                        break;
                    }
                }
            });

            // Keep the runtime alive for as long as the connection exists
            futures::future::pending::<()>().await;
        });
    });

    SyncSimClient::try_new(contributing, to_worker_tx, to_app_rx, join)
}

/// Connect to a **local** server (in-proc, no gRPC, goes through the Coordinator).
pub fn connect_local<S: Simulation>(
    contributing: bool,
    sv: &Arc<SimServer<S>>,
) -> Result<SyncSimClient> {
    let server = &Arc::clone(&sv);

    let coord = server.coord.clone();

    let (to_worker_tx, to_worker_rx) = xchan::bounded::<ClientMsg>(1024);
    let (to_app_tx, to_app_rx) = xchan::bounded::<ServerMsg>(1024);

    // spawn a worker thread that translates ClientMsg <-> Coordinator actions
    let join = thread::spawn(move || {
        let mut _maybe_client_id: Option<u64> = None;
        let mut _out_rx: Option<xchan::Receiver<ServerMsg>> = None;

        for msg in to_worker_rx.iter() {
            match msg.body {
                // Register: handled directly by coordinator
                Some(ClientMsgBody::Register(req)) => {
                    let (id, rx) = coord.register(req.contributing);
                    _maybe_client_id = Some(id);
                    _out_rx = Some(rx);

                    // Send Registered response immediately
                    let _ = to_app_tx.send(ServerMsg {
                        body: Some(ServerMsgBody::Registered(RegisterResponse {
                            client_id: id,
                        })),
                    });

                    // spawn a listener thread for coordinator -> client messages
                    let to_app_tx_inner = to_app_tx.clone();
                    let rx_inner = _out_rx.as_ref().unwrap().clone();
                    thread::spawn(move || {
                        for m in rx_inner.iter() {
                            let _ = to_app_tx_inner.send(m);
                        }
                    });
                }

                // StepReady: handled by coordinator barrier
                Some(ClientMsgBody::StepReady(_sr)) => {
                    coord.step_ready(msg.client_id);
                }

                // Other messages: delegate to simulation handler via coordinator
                _ => {
                    coord.send_message(msg);
                }
            }
        }
    });

    SyncSimClient::try_new(contributing, to_worker_tx, to_app_rx, join)
}

fn spawn_step_thread(work_ms: u64, sim: SyncSimClient) -> thread::JoinHandle<Result<()>> {
    thread::spawn(move || {
        loop {
            sim.step_ready()?;
            sim.wait_for_tick()?;

            std::thread::sleep(Duration::from_millis(work_ms));
        }
    })
}

/// Connect a thread which just calls StepReady on the remote server, acting as a contributing client with no "work".
pub fn connect_remote_stepper(addr: &str, work_ms: u64) -> thread::JoinHandle<Result<()>> {
    let remote_client = connect_remote(true, &addr);
    spawn_step_thread(work_ms, remote_client.expect("connect to remote failed"))
}
