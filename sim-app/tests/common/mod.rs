use std::{net::SocketAddr, time::Duration};

use sim_app::client::{new_client, Client, Mode};
use sim_proto::pb::sim::ServerMsgBody;
use tracing::info;

pub(crate) fn run_sim(
    mut client: Box<dyn Client>,
    app_id: &str,
    frames: usize,
) -> anyhow::Result<()> {
    let mut frame = 0;

    while frame < frames {
        info!("[Client: {app_id}] Frame: {frame}");

        if let Some(msg) = client.recv() {
            match msg.body {
                Some(ServerMsgBody::State(state)) => {
                    info!("[Client: {app_id}] Received ServerMsg: State");
                    std::thread::sleep(Duration::from_millis(500));
                    client.try_step(state.tick)?;
                }
                _ => {
                    panic!("Should not be sending other messages");
                }
            }
        } else {
            panic!("Should be blocking");
        }

        frame += 1;
    }

    info!("[Client: {app_id}] Finished update loop");

    client.join()?;

    Ok(())
}

/// Spawn a remote connector to a hybrid/remote service.
/// - If `contributes` is true, it registers as a contributor and votes StepReady each tick.
/// - If false, it registers as a viewer (non-blocking) and only listens.
pub(crate) fn spawn_remote_connector(
    addr: SocketAddr,
    app_id: &str,
    contributes: bool,
    frames: usize,
) -> std::thread::JoinHandle<anyhow::Result<()>> {
    let app_id = app_id.to_string();
    std::thread::spawn(move || -> anyhow::Result<()> {
        // tiny retry to handle bind/connect races on CI
        let mut attempts = 0;
        let mut client = loop {
            match new_client(Mode::Remote, contributes, &app_id, Some(addr)) {
                Ok(c) => break c,
                Err(_) if attempts < 30 => {
                    attempts += 1;
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
                Err(e) => return Err(e),
            }
        };

        let mut frame = 0usize;

        while frame < frames {
            info!("[Client: {app_id}] Frame: {frame}");

            if let Some(msg) = client.recv() {
                match msg.body {
                    Some(ServerMsgBody::State(state)) => {
                        info!("[Client: {app_id}] Received ServerMsg: State");

                        info!("[Client: {app_id}] Rendering...");

                        std::thread::sleep(Duration::from_millis(3000)); // render delay.

                        client.try_step(state.tick)?;
                    }
                    _ => {
                        panic!("Should not be sending other messages");
                    }
                }
            } else {
                panic!("Should be blocking");
            }

            frame += 1;
        }

        info!("[Client: {app_id}] Finished update loop");

        Ok(())
    })
}
