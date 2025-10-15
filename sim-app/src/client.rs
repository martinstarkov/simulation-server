use anyhow::Result;
use crossbeam::channel::{self, Receiver, Sender};
use std::{net::SocketAddr, thread};
use tokio_stream::StreamExt;

use sim_proto::pb::sim::{simulator_api_client::SimulatorApiClient, ClientMsg, ServerMsg};

use crate::{block_on, spawn_local, spawn_local_with_service, CoreIn, LocalAppLink};

fn wrap_local_link(link: LocalAppLink) -> (Sender<ClientMsg>, Receiver<ServerMsg>) {
    let (tx_in, rx_in) = channel::bounded::<ClientMsg>(1024);
    let (tx_out, rx_out) = channel::bounded::<ServerMsg>(1024);

    // Forward client → simulator
    let tx_in_link = link.tx_in.clone();
    thread::spawn(move || {
        for msg in rx_in.iter() {
            let _ = tx_in_link.send(CoreIn::FromClient(msg));
        }
    });

    // Forward simulator → client
    thread::spawn(move || {
        for msg in link.rx_out.iter() {
            let _ = tx_out.send(msg);
        }
    });

    (tx_in, rx_out)
}

/// Simulation mode (Local, Hybrid, or Remote)
#[derive(Clone, Copy, Debug)]
pub enum SimMode {
    Local,
    Hybrid,
    Remote,
}

/// Unified synchronous client interface for all simulator modes.
/// Provides blocking `send()` and `recv()` methods regardless of transport.
pub struct SimClient {
    tx: Sender<ClientMsg>,
    rx: Receiver<ServerMsg>,
    _join: Option<thread::JoinHandle<()>>, // background thread for remote mode
}

impl SimClient {
    /// Construct a synchronous client for the given mode.
    ///
    /// This function blocks until the simulator (local/hybrid/remote) is ready.
    pub fn new(mode: SimMode, address: Option<SocketAddr>) -> Result<Self> {
        match mode {
            // === LOCAL ===
            SimMode::Local => {
                let (link, join) = spawn_local()?;
                let (tx, rx) = wrap_local_link(link);
                Ok(Self {
                    tx,
                    rx,
                    _join: Some(join),
                })
            }

            // === HYBRID ===
            SimMode::Hybrid => {
                let addr = address.unwrap();
                let (link, join) = spawn_local_with_service(addr)?;
                let (tx, rx) = wrap_local_link(link);
                Ok(Self {
                    tx,
                    rx,
                    _join: Some(join),
                })
            }

            // === REMOTE ===
            SimMode::Remote => {
                let addr = address.unwrap();

                let (tx_client_to_remote, rx_client_to_remote) =
                    channel::bounded::<ClientMsg>(1024);
                let (tx_remote_to_client, rx_remote_to_client) =
                    channel::bounded::<ServerMsg>(1024);

                let join = thread::spawn(move || {
                    block_on(async move {
                        let mut client =
                            match SimulatorApiClient::connect(format!("http://{addr}")).await {
                                Ok(c) => c,
                                Err(e) => {
                                    eprintln!("[SimClient] connect failed {addr}: {e}");
                                    return;
                                }
                            };

                        // client → server stream (tokio mpsc)
                        let (tx_req, rx_req) = tokio::sync::mpsc::channel::<ClientMsg>(128);
                        let outbound = tokio_stream::wrappers::ReceiverStream::new(rx_req);

                        // start bidi stream
                        let response = match client.link(outbound).await {
                            Ok(r) => r,
                            Err(e) => {
                                eprintln!("[SimClient] link failed: {e}");
                                return;
                            }
                        };
                        let mut rx_stream = response.into_inner();

                        // BRIDGE 1: crossbeam (sync) -> tokio mpsc (gRPC outbound)
                        {
                            let tx_req_clone = tx_req.clone();
                            let rx_sync = rx_client_to_remote.clone();
                            std::thread::spawn(move || {
                                for msg in rx_sync.iter() {
                                    // IMPORTANT: use blocking_send from a blocking thread
                                    if tx_req_clone.blocking_send(msg).is_err() {
                                        break;
                                    }
                                }
                            });
                        }

                        // BRIDGE 2: gRPC inbound -> crossbeam (sync) for user recv()
                        while let Some(Ok(msg)) = rx_stream.next().await {
                            let _ = tx_remote_to_client.send(msg);
                        }
                    });
                });

                Ok(Self {
                    tx: tx_client_to_remote,
                    rx: rx_remote_to_client,
                    _join: Some(join),
                })
            }
        }
    }

    /// Send a message to the simulator (blocking).
    pub fn send(&self, msg: ClientMsg) -> Result<()> {
        println!("Sending client message to server");
        self.tx.send(msg)?;
        Ok(())
    }

    /// Receive the next message from the simulator (blocking).
    pub fn recv(&self) -> Option<ServerMsg> {
        println!("Receiving server msg on client");
        self.rx.recv().ok()
    }
}
