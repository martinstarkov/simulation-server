use anyhow::Result;
use std::{net::SocketAddr, pin::Pin, sync::Arc, time::Duration};

use futures_core::Stream;
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::{wrappers::BroadcastStream, StreamExt};
use tonic::{Request, Response, Status};

use sim_proto::pb::sim::{
    sim_msg::Kind,
    simulator_api_server::{SimulatorApi, SimulatorApiServer},
    Ack, SimMsg,
};

/// Minimal simulator core.
pub struct SimulatorCore {
    cmd_rx: mpsc::Receiver<SimMsg>,
    state_tx: broadcast::Sender<SimMsg>,
    allow_shutdown: bool,
}

impl SimulatorCore {
    pub fn new(
        cmd_rx: mpsc::Receiver<SimMsg>,
        state_tx: broadcast::Sender<SimMsg>,
        allow_shutdown: bool,
    ) -> Self {
        Self {
            cmd_rx,
            state_tx,
            allow_shutdown,
        }
    }

    pub async fn run(mut self) {
        let mut ticker = tokio::time::interval(Duration::from_millis(500));
        let mut tick = 0u64;

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    tick += 1;
                    let _ = self.state_tx.send(SimMsg { kind: Some(Kind::State(format!("state:{tick}"))) });
                }
                maybe = self.cmd_rx.recv() => {
                    match maybe {
                        Some(msg) => match msg.kind {
                            Some(Kind::Command(c)) => {
                                let _ = self.state_tx.send(SimMsg { kind: Some(Kind::State(format!("ack:{c}"))) });
                            }
                            Some(Kind::Tick(t)) => {
                                tick = t;
                                let _ = self.state_tx.send(SimMsg { kind: Some(Kind::State(format!("retick:{tick}"))) });
                            }
                            Some(Kind::Shutdown(true)) => {
                                if self.allow_shutdown {
                                    let _ = self.state_tx.send(SimMsg { kind: Some(Kind::State("shutdown".into())) });
                                    break;
                                } else {
                                    // Ignore remote shutdowns in service mode
                                    let _ = self.state_tx.send(SimMsg { kind: Some(Kind::State("shutdown_ignored".into())) });
                                }
                            }
                            _ => {}
                        },
                        None => break,
                    }
                }
            }
        }
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

    // Tonic wants a Stream<Item = Result<SimMsg, Status>>
    type SubscribeStream = Pin<Box<dyn Stream<Item = Result<SimMsg, Status>> + Send + 'static>>;

    async fn subscribe(
        &self,
        _req: tonic::Request<sim_proto::pb::sim::Empty>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        let rx = self.bp.state_tx.subscribe();

        // Map Result<SimMsg, BroadcastStreamRecvError> -> Result<SimMsg, Status>
        let stream = BroadcastStream::new(rx).map(|res| match res {
            Ok(msg) => Ok(msg),
            // If a client lags, emit a marker instead of erroring out
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

/// Spawn simulator locally (channels), optionally also expose gRPC for remote clients.
pub async fn spawn_local(
    enable_remote_client: bool,
    rc_addr: Option<SocketAddr>,
) -> Result<(AppLink, tokio::task::JoinHandle<()>)> {
    let (cmd_tx, cmd_rx) = mpsc::channel::<SimMsg>(1024);
    let (state_tx, _rx0) = broadcast::channel::<SimMsg>(2048);

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

    // Core runner
    let core = SimulatorCore::new(cmd_rx, state_tx.clone(), true);
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
    let (state_tx, _rx0) = broadcast::channel::<SimMsg>(2048);

    // clone before move so we still own a Sender for the Backplane
    let state_tx_core = state_tx.clone();
    tokio::spawn(async move {
        SimulatorCore::new(cmd_rx, state_tx_core, false).run().await;
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
