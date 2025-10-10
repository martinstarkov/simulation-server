use crate::{spawn_local, spawn_local_with_service, LocalAppLink, STATE_WAIT_INTERVAL};
use anyhow::Result;
use clap::{Parser, ValueEnum};
use sim_proto::pb::sim::ServerMsgBody;
use sim_proto::pb::sim::{
    simulator_api_client::SimulatorApiClient, ClientMsg, ClientMsgBody, Register, ServerMsg,
    StepReady,
};
use std::net::SocketAddr;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

/// How to connect to the simulator.
#[derive(ValueEnum, Clone, Debug, Default)]
pub enum Mode {
    #[default]
    Local,
    Hybrid,
    Remote,
}

/// Shared CLI arguments (optional, but convenient)
#[derive(Parser, Debug, Default)]
pub struct Args {
    #[arg(long, value_enum, default_value = "local")]
    pub mode: Mode,

    #[arg(long, default_value = "127.0.0.1:50051")]
    pub addr: String,

    #[arg(long, default_value = "app-1")]
    pub app_id: String,
}

/// Abstracted simulator client — local, hybrid, or remote.

pub struct ClientSync {
    id: String,
    link: LocalAppLink,
}

impl ClientSync {
    pub fn new(id: &str) -> Result<Self> {
        let (link, _) = spawn_local()?;
        Ok(Self {
            id: id.into(),
            link,
        })
    }

    pub fn send(&self, msg: ClientMsg) {
        self.link.send(msg);
    }

    pub fn recv(&self) -> Option<ServerMsgBody> {
        self.link.next()?.body
    }

    pub fn run<F>(&mut self, mut on_msg: F) -> Result<()>
    where
        F: FnMut(ServerMsgBody) -> Result<()>,
    {
        while let Some(msg) = self.recv() {
            on_msg(msg)?;
        }
        Ok(())
    }

    /// Register as a contributing client.
    pub fn register(&self) -> Result<()> {
        self.send(ClientMsg {
            app_id: self.id.clone(),
            body: Some(ClientMsgBody::Register(Register { contributes: true })),
        });
        Ok(())
    }

    /// Mark step ready.
    pub fn ready(&self, tick: u64) -> Result<()> {
        self.send(ClientMsg {
            app_id: self.id.clone(),
            body: Some(ClientMsgBody::StepReady(StepReady { tick })),
        });
        Ok(())
    }
}

pub struct Client {
    pub id: String,
    inner: ClientInner,
}

enum ClientInner {
    Local {
        link: LocalAppLink,
    },
    Remote {
        tx_req: mpsc::Sender<ClientMsg>,
        rx: tonic::Streaming<ServerMsg>,
    },
}

impl Client {
    /// Build and connect based on mode.
    pub async fn connect(args: &Args) -> Result<Self> {
        let id = args.app_id.clone();

        match args.mode {
            Mode::Local => {
                let (link, _join) = spawn_local()?; // User controls lifecycle
                Ok(Self {
                    id,
                    inner: ClientInner::Local { link },
                })
            }
            Mode::Hybrid => {
                let addr: SocketAddr = args.addr.parse()?;
                let (link, _join) = spawn_local_with_service(addr).await?;
                Ok(Self {
                    id,
                    inner: ClientInner::Local { link },
                })
            }
            Mode::Remote => {
                let addr: SocketAddr = args.addr.parse()?;
                let mut client = SimulatorApiClient::connect(format!("http://{addr}")).await?;
                let (tx_req, rx_req) = mpsc::channel::<ClientMsg>(128);
                let outbound = ReceiverStream::new(rx_req);
                let rx = client.link(outbound).await?.into_inner();

                Ok(Self {
                    id,
                    inner: ClientInner::Remote { tx_req, rx },
                })
            }
        }
    }

    /// Send a message to the simulator.
    pub async fn send(&self, msg: ClientMsg) -> Result<()> {
        match &self.inner {
            ClientInner::Local { link } => {
                link.send(msg);
                Ok(())
            }
            ClientInner::Remote { tx_req, .. } => {
                tx_req.send(msg).await?;
                Ok(())
            }
        }
    }

    /// Receive the next message from the simulator (async).
    pub async fn recv(&mut self) -> Option<ServerMsg> {
        match &mut self.inner {
            ClientInner::Local { link } => link.next(),
            ClientInner::Remote { rx, .. } => {
                match tokio::time::timeout(STATE_WAIT_INTERVAL, rx.next()).await {
                    Ok(Some(Ok(msg))) => Some(msg),
                    _ => None,
                }
            }
        }
    }

    /// Generic async run loop over `ServerMsgBody` variants.
    pub async fn run<F, Fut>(&mut self, mut on_msg: F) -> Result<()>
    where
        F: FnMut(ServerMsgBody) -> Fut,
        Fut: std::future::Future<Output = Result<()>>,
    {
        while let Some(msg) = self.recv().await {
            if let Some(body) = msg.body {
                on_msg(body).await?;
            }
        }
        Ok(())
    }

    /// Register as a contributing client.
    pub async fn register(&self) -> Result<()> {
        self.send(ClientMsg {
            app_id: self.id.clone(),
            body: Some(ClientMsgBody::Register(Register { contributes: true })),
        })
        .await
    }

    /// Mark step ready.
    pub async fn ready(&self, tick: u64) -> Result<()> {
        self.send(ClientMsg {
            app_id: self.id.clone(),
            body: Some(ClientMsgBody::StepReady(StepReady { tick })),
        })
        .await
    }
}
