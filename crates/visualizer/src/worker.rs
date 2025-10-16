use anyhow::Result;
use client::sync_client::SyncSimClient;
use crossbeam_channel as xchan;
use interface::interface::{
    ClientMsg, RegisterRequest, RegisterResponse, ServerMsg, StepReady, client_msg, server_msg,
};
use std::thread;

pub fn spawn_viewer_worker(
    name: &str,
    contributing: bool,
    sim: SyncSimClient,
) -> (xchan::Receiver<ServerMsg>, xchan::Sender<()>) {
    let (tx_app, rx_app) = xchan::bounded::<ServerMsg>(1024);
    let (tx_ready, rx_ready) = xchan::bounded::<()>(1);
    let name = name.to_string();

    thread::Builder::new()
        .name(name.clone())
        .spawn(move || -> anyhow::Result<()> {
            // 1. Register with the server
            sim.send(ClientMsg {
                msg: Some(client_msg::Msg::Register(RegisterRequest {
                    client_name: name.clone(),
                    contributing,
                })),
            })?;
            println!("[Viewer: {name}] Registering with server...");

            let client_id = loop {
                match sim.recv()? {
                    ServerMsg {
                        msg: Some(server_msg::Msg::Registered(RegisterResponse { client_id })),
                    } => break client_id,
                    _ => {}
                }
            };
            println!("[Viewer: {client_id}] Registered!");

            let mut tick_seq = 0;

            loop {
                if contributing {
                    // Tell server we’re ready for next step
                    sim.send(ClientMsg {
                        msg: Some(client_msg::Msg::StepReady(StepReady {
                            client_id,
                            tick_seq,
                        })),
                    })?;

                    // Block until Bevy signals "frame done"
                    rx_ready.recv().ok();
                }

                // Wait for next tick & state
                loop {
                    match sim.recv() {
                        Ok(msg) => {
                            let _ = tx_app.send(msg.clone());

                            if let Some(server_msg::Msg::Tick(t)) = msg.msg {
                                tick_seq = t.seq;
                                println!("[Viewer: {client_id}] Got Tick {}", tick_seq);
                                break; // got the next tick → let Bevy render
                            }
                        }
                        Err(e) => {
                            eprintln!("Connection error: {e:?}");
                            return Ok(());
                        }
                    }
                }

                // forward any additional state messages immediately
                while let Some(m) = sim.try_recv() {
                    let _ = tx_app.send(m);
                }
            }
        })
        .expect("spawn viewer worker thread");

    (rx_app, tx_ready)
}
