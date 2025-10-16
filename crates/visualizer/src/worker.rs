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
    let (tx_done, rx_done) = xchan::bounded::<()>(1);
    let name = name.to_string();

    thread::Builder::new()
        .name(name.clone())
        .spawn(move || -> Result<()> {
            // 1) Register
            sim.send(ClientMsg {
                msg: Some(client_msg::Msg::Register(RegisterRequest {
                    client_name: name.clone(),
                    contributing,
                })),
            })?;

            println!("[Viewer: {name}] Registering with the server...");

            // 2) Wait for Registered
            let client_id = loop {
                match sim.recv()? {
                    ServerMsg {
                        msg: Some(server_msg::Msg::Registered(RegisterResponse { client_id })),
                    } => break client_id,
                    _ => {}
                }
            };

            println!("[Viewer: {client_id}] Registered with server");

            let mut tick_seq = 0;

            loop {
                if contributing {
                    // Wait for Bevy to finish the current frame before sending StepReady
                    match rx_done.recv() {
                        Ok(()) => {
                            sim.send(ClientMsg {
                                msg: Some(client_msg::Msg::StepReady(StepReady {
                                    client_id,
                                    tick_seq,
                                })),
                            })?;
                            println!(
                                "[Viewer: {client_id}] StepReady sent after Bevy frame {}",
                                tick_seq
                            );
                        }
                        Err(_) => {
                            eprintln!("[Viewer: {client_id}] Bevy channel closed; exiting");
                            return Ok(());
                        }
                    }
                }

                // Wait for next Tick (and forward all messages)
                loop {
                    match sim.recv() {
                        Ok(msg) => {
                            let _ = tx_app.send(msg.clone());

                            if let Some(server_msg::Msg::Tick(t)) = msg.msg {
                                tick_seq = t.seq;
                                println!(
                                    "[Viewer: {client_id}] received Tick {} (frame ready)",
                                    tick_seq
                                );
                                break;
                            }
                        }
                        Err(e) => {
                            eprintln!("[Viewer: {client_id}] connection error: {e:?}");
                            return Ok(());
                        }
                    }
                }

                // Drain any leftover messages (like State)
                while let Some(msg) = sim.try_recv() {
                    let _ = tx_app.send(msg);
                }
            }
        })
        .expect("spawn viewer worker thread");

    (rx_app, tx_done)
}
