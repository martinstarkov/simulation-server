// client.rs
use anyhow::{anyhow, Result};
use std::{net::SocketAddr, thread};
use tracing::info;

use sim_proto::pb::sim::{
    simulator_api_client::SimulatorApiClient, ClientMsg, ClientMsgBody, Register, ServerMsg,
    StepReady,
};

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use crate::{block_on, spawn_hybrid, spawn_local, tick_tracker::TickTracker, LocalAppLink};

// =====================
// Trait
// =====================

pub trait Client: Send {
    fn send(&self, msg: ClientMsg) -> Result<()>;
    fn recv(&mut self) -> Option<ServerMsg>;
    fn join(self: Box<Self>) -> Result<()>;

    fn app_id(&self) -> &String;

    fn step(&self, tick: u64) -> Result<()> {
        if !self.contributes() {
            return Ok(());
        }
        let app_id = self.app_id().clone();
        info!("[Client: {app_id}] Sending StepReady: {tick}");
        self.send(ClientMsg {
            app_id,
            body: Some(ClientMsgBody::StepReady(StepReady { tick })),
        })
    }

    fn contributes(&self) -> bool;

    fn set_contributes(&mut self, contributes: bool);

    fn register(&mut self, contributes: bool) -> Result<()> {
        let app_id = self.app_id().clone();
        info!("[Client: {app_id}] Sending Register");
        self.set_contributes(contributes);
        self.send(ClientMsg {
            app_id,
            body: Some(ClientMsgBody::Register(Register { contributes })),
        })
    }

    fn tick_tracker(&mut self) -> &mut TickTracker;

    fn try_step(&mut self, tick: u64) -> Result<()> {
        let app_id = self.app_id().clone();
        if let Some(t) = self.tick_tracker().update_with(&app_id, tick) {
            self.step(t)?;
        }
        Ok(())
    }
}

// Optional: a factory enum
#[derive(Clone, Copy, Debug)]
pub enum Mode {
    Local,
    Hybrid, // requires an addr
    Remote, // requires an addr
}

pub fn new_client(
    mode: Mode,
    contributes: bool,
    app_id: &str,
    addr: Option<SocketAddr>,
) -> Result<Box<dyn Client>> {
    let c: Result<Box<dyn Client>> = match mode {
        Mode::Local => Ok(Box::new(LocalClient::new(app_id)?)),
        Mode::Hybrid => Ok(Box::new(HybridClient::new(
            app_id,
            addr.expect("Hybrid needs addr"),
        )?)),
        Mode::Remote => Ok(Box::new(RemoteClient::new(
            app_id,
            addr.expect("Remote needs addr"),
        )?)),
    };

    let mut client = c?;

    client.register(contributes)?;

    if contributes {
        client.step(0)?;
    }

    Ok(client)
}

// =====================
// Local client
// =====================

pub struct LocalClient {
    link: LocalAppLink,
    core: thread::JoinHandle<()>, // simulator core running locally
    tick_tracker: TickTracker,
    contributes: bool,
}

impl LocalClient {
    pub fn new(app_id: &str) -> Result<Self> {
        info!("[Client: {app_id}] Creating LocalClient");
        let (link, core) = spawn_local(app_id)?;
        Ok(Self {
            link,
            core,
            tick_tracker: TickTracker::default(),
            contributes: false,
        })
    }
}

impl Client for LocalClient {
    fn send(&self, msg: ClientMsg) -> Result<()> {
        self.link.send(msg);
        Ok(())
    }

    fn recv(&mut self) -> Option<ServerMsg> {
        // LocalAppLink::next() blocks on crossbeam::Receiver::recv()
        self.link.next()
    }

    fn app_id(&self) -> &String {
        &self.link.app_id
    }

    fn tick_tracker(&mut self) -> &mut TickTracker {
        &mut self.tick_tracker
    }

    fn contributes(&self) -> bool {
        self.contributes
    }

    fn set_contributes(&mut self, contributes: bool) {
        self.contributes = contributes;
    }

    fn join(self: Box<Self>) -> Result<()> {
        self.link.shutdown();
        info!("[Client: {}] Joining core server thread", self.link.app_id);
        self.core
            .join()
            .map_err(|_| anyhow!("Core thread failed to join"))
    }
}

// =====================
// Hybrid client
// (Local channel path, but also starts a gRPC service for external viewers)
// =====================

pub struct HybridClient {
    link: LocalAppLink,
    core: thread::JoinHandle<()>, // simulator core
    svc: thread::JoinHandle<()>,  // gRPC service running locally
    svc_shutdown: tokio::sync::watch::Sender<bool>,
    tick_tracker: TickTracker,
    contributes: bool,
}

impl HybridClient {
    pub fn new(app_id: &str, addr: SocketAddr) -> Result<Self> {
        info!("[Client: {app_id}] Creating HybridClient");
        let (link, core, svc, svc_shutdown) = spawn_hybrid(app_id, addr)?;
        Ok(Self {
            link,
            core,
            svc,
            svc_shutdown,
            tick_tracker: TickTracker::default(),
            contributes: false,
        })
    }
}

impl Client for HybridClient {
    fn send(&self, msg: ClientMsg) -> Result<()> {
        self.link.send(msg);
        Ok(())
    }

    fn recv(&mut self) -> Option<ServerMsg> {
        self.link.next()
    }

    fn app_id(&self) -> &String {
        &self.link.app_id
    }

    fn contributes(&self) -> bool {
        self.contributes
    }

    fn set_contributes(&mut self, contributes: bool) {
        self.contributes = contributes;
    }

    fn tick_tracker(&mut self) -> &mut TickTracker {
        &mut self.tick_tracker
    }

    fn join(self: Box<Self>) -> Result<()> {
        let _ = self.svc_shutdown.send(true);
        info!("[Client: {}] Joining service thread", self.link.app_id);
        self.svc
            .join()
            .map_err(|_| anyhow!("Service thread failed to join"))?;

        self.link.shutdown();
        info!("[Client: {}] Joining core server thread", self.link.app_id);
        self.core
            .join()
            .map_err(|_| anyhow!("Core thread failed to join"))?;

        Ok(())
    }
}

// =====================
// Remote client
// (Background thread runs tokio + tonic bidi; sync facade via blocking_*)
// =====================

pub struct RemoteClient {
    app_id: String,
    tx: mpsc::Sender<ClientMsg>,
    rx: mpsc::Receiver<ServerMsg>,
    remote: thread::JoinHandle<()>,
    tick_tracker: TickTracker,
    contributes: bool,
}

impl RemoteClient {
    pub fn new(app_id: &str, addr: SocketAddr) -> Result<Self> {
        info!("[Client: {app_id}] Creating RemoteClient");
        // channels for the sync facade
        let (tx_to_rt, rx_from_user) = mpsc::channel::<ClientMsg>(256);
        let (tx_to_user, rx_from_rt) = mpsc::channel::<ServerMsg>(256);

        let app_id_thread = app_id.to_string();

        let remote = thread::spawn(move || {
            block_on(async move {
                // connect tonic client
                let mut grpc = match SimulatorApiClient::connect(format!("http://{addr}")).await {
                    Ok(c) => c,
                    Err(e) => {
                        info!("[Client: {app_id_thread}] Remote connection failed {addr}: {e}");
                        return;
                    }
                };

                // user -> server: stream driven by rx_from_user
                let outbound = ReceiverStream::new(rx_from_user);

                // open bidi stream
                let mut inbound = match grpc.link(outbound).await {
                    Ok(resp) => resp.into_inner(),
                    Err(e) => {
                        info!("[Client: {app_id_thread}] remote link failed: {e}");
                        return;
                    }
                };

                // server -> user
                while let Some(Ok(msg)) = inbound.next().await {
                    if tx_to_user.send(msg).await.is_err() {
                        break; // user dropped receiver
                    }
                }
            });
        });

        Ok(Self {
            app_id: app_id.to_string(),
            tx: tx_to_rt,
            rx: rx_from_rt,
            remote,
            tick_tracker: TickTracker::default(),
            contributes: false,
        })
    }
}

impl Client for RemoteClient {
    fn send(&self, msg: ClientMsg) -> Result<()> {
        //info!("[Client: {}]: Sending ClientMsg", self.app_id);
        // Requires tokio's "sync" feature for blocking_send
        self.tx
            .blocking_send(msg)
            .map_err(|e| anyhow!("Remote client send failed: {e}"))
    }

    fn app_id(&self) -> &String {
        &self.app_id
    }

    fn tick_tracker(&mut self) -> &mut TickTracker {
        &mut self.tick_tracker
    }

    fn contributes(&self) -> bool {
        self.contributes
    }

    fn set_contributes(&mut self, contributes: bool) {
        self.contributes = contributes;
    }

    fn recv(&mut self) -> Option<ServerMsg> {
        // Requires tokio's "sync" feature for blocking_recv
        let msg = self.rx.blocking_recv();
        // if msg.is_some() {
        //     info!("[Client: {}] Receiving ServerMsg", self.app_id);
        // }
        msg
    }

    fn join(self: Box<Self>) -> Result<()> {
        info!("[Client: {}] Joining remote thread", self.app_id);

        self.remote
            .join()
            .map_err(|_| anyhow!("Remote thread failed to join"))
    }
}
