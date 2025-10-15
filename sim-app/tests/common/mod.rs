use std::{net::SocketAddr, time::Duration};

use sim_app::client::{new_client, Client, Mode};
use sim_proto::pb::sim::{ClientMsg, ClientMsgBody, Register, ServerMsgBody, StepReady};
use tracing::info;

pub(crate) fn run_sim(
    mut client: Box<dyn Client>,
    app_id: &str,
    n_states: usize,
) -> anyhow::Result<()> {
    info!("[Client: {app_id}] Sending Register");
    client.send(ClientMsg {
        app_id: app_id.into(),
        body: Some(ClientMsgBody::Register(Register { contributes: true })),
    })?;

    let mut last_tick: u64 = 0;

    info!("[Client: {app_id}] Sending StepReady: {last_tick}");
    client.send(ClientMsg {
        app_id: app_id.into(),
        body: Some(ClientMsgBody::StepReady(StepReady { tick: 0 })),
    })?;

    let mut processed = 0;

    while processed < n_states {
        if let Some(msg) = client.recv() {
            if let Some(sim_proto::pb::sim::server_msg::Body::State(state)) = msg.body {
                info!("[Client: {app_id}] Received ServerMsg: State");
                if state.tick > last_tick {
                    last_tick = state.tick;
                    info!("[Client: {app_id}] Updated to tick: {}", last_tick);
                    std::thread::sleep(Duration::from_millis(500));
                    info!("[Client: {app_id}] Sending StepReady: {last_tick}");
                    client.send(ClientMsg {
                        app_id: app_id.into(),
                        body: Some(ClientMsgBody::StepReady(StepReady { tick: last_tick })),
                    })?;
                    processed += 1;
                }
            }
        }
    }

    info!("[Client: {app_id}] Finished update loop");

    client.join()?;

    Ok(())
}

/// Spawn a remote connector to a hybrid/remote service.
/// - If `contributes` is true, it registers as a contributor and votes StepReady each tick.
/// - If false, it registers as a viewer (non-blocking) and only listens.
/// It will process up to `n_states` state messages, then return Ok(()).
pub(crate) fn spawn_remote_connector(
    addr: SocketAddr,
    app_id: &str,
    contributes: bool,
    n_states: usize,
) -> std::thread::JoinHandle<anyhow::Result<()>> {
    let app_id = app_id.to_string();
    std::thread::spawn(move || -> anyhow::Result<()> {
        // tiny retry to handle bind/connect races on CI
        let mut attempts = 0;
        let mut client = loop {
            match new_client(Mode::Remote, &app_id, Some(addr)) {
                Ok(c) => break c,
                Err(_) if attempts < 30 => {
                    attempts += 1;
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
                Err(e) => return Err(e),
            }
        };

        info!("[Client: {app_id}] Sending Register");

        // Register with chosen contributes flag
        client.send(ClientMsg {
            app_id: app_id.clone(),
            body: Some(ClientMsgBody::Register(Register { contributes })),
        })?;

        let mut last_tick = 0u64;

        // If contributing, prime the initial tick 0
        if contributes {
            info!("[Client: {app_id}] Sending StepReady: {last_tick}");
            client.send(ClientMsg {
                app_id: app_id.clone(),
                body: Some(ClientMsgBody::StepReady(StepReady { tick: 0 })),
            })?;
        }

        let mut seen = 0usize;

        while seen < n_states {
            if let Some(msg) = client.recv() {
                if let Some(ServerMsgBody::State(s)) = msg.body {
                    info!("[Client: {app_id}] Received ServerMsg: State");

                    seen += 1;

                    info!("[Client: {app_id}] Rendering...");

                    std::thread::sleep(Duration::from_millis(500)); // render delay.

                    if contributes && s.tick > last_tick {
                        last_tick = s.tick;
                        // vote for the tick we just received

                        info!("[Client: {app_id}] Sending StepReady: {last_tick}");

                        client.send(ClientMsg {
                            app_id: app_id.clone(),
                            body: Some(ClientMsgBody::StepReady(StepReady { tick: last_tick })),
                        })?;
                    }
                }
            }
        }

        Ok(())
    })
}
