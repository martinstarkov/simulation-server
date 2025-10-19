use anyhow::Result;
use sim_app::client::Mode;
use std::time::{Duration, Instant};

use sim_proto::pb::sim::{server_msg::Body as SBody, ServerMsg};

use crossbeam::channel;
use std::{net::SocketAddr, thread};

use sim_app::client::new_client;
use sim_proto::pb::sim::{ClientMsg, ClientMsgBody, Register, StepReady};

pub struct NetHandle {
    rx_server: channel::Receiver<ServerMsg>, // network → game
    tx_client: channel::Sender<ClientMsg>,   // game → network (raw ClientMsg)
    join: thread::JoinHandle<Result<(), anyhow::Error>>,
}

impl NetHandle {
    /// Non-blocking: drain all queued server messages
    pub fn poll(&self, mut f: impl FnMut(ServerMsg)) {
        while let Ok(m) = self.rx_server.try_recv() {
            f(m)
        }
    }

    /// Convenience: send StepReady(tick) without building the message yourself
    pub fn step_ready(&self, app_id: &str, tick: u64) {
        let _ = self.tx_client.send(ClientMsg {
            app_id: app_id.to_string(),
            body: Some(ClientMsgBody::StepReady(StepReady { tick })),
        });
    }

    /// Send any custom ClientMsg (e.g., Request)
    pub fn send(&self, msg: ClientMsg) {
        let _ = self.tx_client.send(msg);
    }

    pub fn shutdown(self) {
        let _ = self.join.join();
    }
}

/// Spawns the blocking client on a thread. You can push `ClientMsg` straight to it.
pub fn start_network_simple(
    mode: Mode,
    contributes: bool,
    app_id: &str,
    addr: Option<SocketAddr>,
    prime_tick0: bool,
) -> Result<NetHandle> {
    let (tx_client, rx_client) = channel::unbounded::<ClientMsg>();
    let (tx_server, rx_server) = channel::unbounded::<ServerMsg>();
    let app_id = app_id.to_string();

    let join = thread::spawn(move || -> Result<()> {
        let mut client = new_client(mode, contributes, &app_id, addr)?;

        // Register (idempotent if your factory already does it)
        client.send(ClientMsg {
            app_id: app_id.clone(),
            body: Some(ClientMsgBody::Register(Register { contributes })),
        })?;
        if contributes && prime_tick0 {
            client.send(ClientMsg {
                app_id: app_id.clone(),
                body: Some(ClientMsgBody::StepReady(StepReady { tick: 0 })),
            })?;
        }

        loop {
            // 1) forward any pending ClientMsg from game → network
            while let Ok(msg) = rx_client.try_recv() {
                client.send(msg)?;
            }

            // 2) BLOCK on next server message, then forward to game
            match client.recv() {
                Some(msg) => {
                    let _ = tx_server.send(msg);
                }
                None => {
                    client.join()?;
                    return Ok(());
                }
            }
        }
    });

    Ok(NetHandle {
        rx_server,
        tx_client,
        join,
    })
}

/// Track newest tick seen from any ServerMsg.
#[derive(Default)]
pub struct MsgTickTracker {
    last: u64,
}
impl MsgTickTracker {
    pub fn update_from(&mut self, msg: &ServerMsg) -> Option<u64> {
        if let Some(SBody::State(s)) = &msg.body {
            if s.tick > self.last {
                self.last = s.tick;
                return Some(s.tick);
            }
        }
        None
    }
    pub fn last(&self) -> u64 {
        self.last
    }
}

fn main() -> Result<()> {
    let app_id = "game-client-1";
    let net = start_network_simple(
        Mode::Local, // or Local / Hybrid
        true,        // contributes
        app_id,
        Some("127.0.0.1:60000".parse().unwrap()),
        true, // prime tick 0
    )?;

    let mut ticks = MsgTickTracker::default();
    let mut pending: Option<u64> = None;

    // fixed timestep (≈60 FPS)
    let dt = Duration::from_millis(16);
    let mut next = Instant::now();

    'run: loop {
        // 1) Non-blocking poll of server messages
        net.poll(|msg| {
            if let Some(t) = ticks.update_from(&msg) {
                // Apply to your world first if you want…
                pending = Some(t); // remember the newest tick to vote on this frame
            }
        });

        // 2) Update your world here (physics, AI, render prep, etc.)

        // 3) Decide when to StepReady (here: immediately if we saw a new tick)
        if let Some(tick) = pending.take() {
            // Optional pacing:
            // std::thread::sleep(Duration::from_millis(500));
            net.step_ready(app_id, tick);
        }

        // 4) Frame pacing
        let now = Instant::now();
        if now < next {
            std::thread::sleep(next - now);
        }
        next += dt;

        // exit condition for demo
        if ticks.last() >= 5 {
            break 'run;
        }
    }

    net.shutdown();
    Ok(())
}
