use std::{
    sync::Arc,
    thread::{self, JoinHandle},
};

use anyhow::{Result, anyhow};
use crossbeam_channel as xchan;

use interface::{
    Simulation,
    interface::{ClientMsg, ServerMsg, server_msg, simulator_client::SimulatorClient},
};

use server::server::{SimServer, safe_block_on};
use tracing::{error, info};

// ===== Public sync facade =====================================================

/// Transport-agnostic, synchronous client handle.
pub struct SyncSimClient {
    tx_to_worker: xchan::Sender<ClientMsg>,
    rx_from_worker: xchan::Receiver<ServerMsg>,
    _join: JoinHandle<()>,
}

impl SyncSimClient {
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
}

/// Connect to a **remote** gRPC server at the given address.
///
/// You can pass either:
/// - `"127.0.0.1:50051"` → automatically becomes `"http://127.0.0.1:50051"`
/// - `"http://127.0.0.1:50051"` → used as is
/// Connect to a **remote** gRPC server at the given address.
pub fn connect_remote(addr: &str) -> Result<SyncSimClient> {
    let (to_worker_tx, to_worker_rx) = xchan::bounded::<ClientMsg>(1024);
    let (to_app_tx, to_app_rx) = xchan::bounded::<ServerMsg>(1024);

    // Automatically prepend "http://" if not present
    let addr_http = if addr.starts_with("http://") || addr.starts_with("https://") {
        addr.to_string()
    } else {
        format!("http://{}", addr)
    };

    info!("Remote client connecting to {addr_http}");

    let join = std::thread::spawn(move || {
        // Each worker thread runs its own lightweight single-threaded runtime
        safe_block_on(async move {
            // --- 1) Connect to gRPC server ---
            let mut grpc = match SimulatorClient::connect(addr_http.clone()).await {
                Ok(c) => {
                    info!("Remote client successfully connected to gRPC on {addr_http}");
                    c
                }
                Err(e) => {
                    error!("Remote client failed to connect: {e}");
                    let _ = to_app_tx.send(ServerMsg {
                        msg: Some(server_msg::Msg::ErrorMsg(interface::interface::ErrorMsg {
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
                        msg: Some(server_msg::Msg::ErrorMsg(interface::interface::ErrorMsg {
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

    Ok(SyncSimClient {
        tx_to_worker: to_worker_tx,
        rx_from_worker: to_app_rx,
        _join: join,
    })
}

/// Connect to a **local** server (in-proc, no gRPC, goes through the Coordinator).
pub fn connect_local<S: Simulation>(server: &Arc<SimServer<S>>) -> Result<SyncSimClient> {
    let coord = server.coord.clone();

    let (to_worker_tx, to_worker_rx) = xchan::bounded::<ClientMsg>(1024);
    let (to_app_tx, to_app_rx) = xchan::bounded::<ServerMsg>(1024);

    // spawn a worker thread that translates ClientMsg <-> Coordinator actions
    let join = thread::Builder::new()
        .name("sync-local-worker".into())
        .spawn(move || {
            let mut _maybe_client_id: Option<u64> = None;
            let mut _out_rx: Option<xchan::Receiver<ServerMsg>> = None;

            for msg in to_worker_rx.iter() {
                match msg.msg {
                    // Register: handled directly by coordinator
                    Some(interface::interface::client_msg::Msg::Register(req)) => {
                        let (id, rx) = coord.register(&req.client_name, req.contributing);
                        _maybe_client_id = Some(id);
                        _out_rx = Some(rx);

                        // Send Registered response immediately
                        let _ = to_app_tx.send(ServerMsg {
                            msg: Some(interface::interface::server_msg::Msg::Registered(
                                interface::interface::RegisterResponse { client_id: id },
                            )),
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
                    Some(interface::interface::client_msg::Msg::StepReady(sr)) => {
                        coord.step_ready(sr.client_id);
                    }

                    // Other messages: delegate to simulation handler via coordinator
                    _ => {
                        coord.send_message(msg);
                    }
                }
            }
        })?;

    Ok(SyncSimClient {
        tx_to_worker: to_worker_tx,
        rx_from_worker: to_app_rx,
        _join: join,
    })
}
