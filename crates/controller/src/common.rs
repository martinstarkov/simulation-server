use std::thread;
use std::time::Duration;

use anyhow::Result;
use client::sync_client::SyncSimClient;
use interface::interface::{
    ClientMsg, RegisterRequest, RegisterResponse, ServerMsg, StepReady, client_msg, server_msg,
};

/// Start one blocking "controller" thread:
/// - connects via the provided closure (local or remote)
/// - Registers (contributing)
/// - For `cycles` times:
///     * does some work (sleep)
///     * sends StepReady(client_id, 0)
///     * blocks until it sees a Tick (ignores other msgs)
pub fn spawn_controller_thread(
    name: &str,
    cycles: usize,
    work_ms: u64,
    sim: SyncSimClient,
) -> thread::JoinHandle<Result<()>> {
    let name = name.to_string();
    thread::Builder::new()
        .name(name.clone())
        .spawn(move || -> Result<()> {
            // 1) Register (contributing)
            sim.send(ClientMsg {
                msg: Some(client_msg::Msg::Register(RegisterRequest {
                    client_name: name.clone(),
                    contributing: true,
                })),
            })?;

            println!("[Client: {name}] Registering with the server...");

            // 2) Wait for Registered to get client_id
            let client_id = loop {
                match sim.recv()? {
                    ServerMsg {
                        msg: Some(server_msg::Msg::Registered(RegisterResponse { client_id })),
                    } => break client_id,
                    _ => {} // ignore until we see Registered
                }
            };

            println!(
                "[Client: {client_id}] Registered with the server and received RegisterResponse"
            );

            #[allow(unused_assignments)]
            let mut tick: u64 = 0;

            // 3) Main step loop
            for _ in 0..cycles {
                // tell server you're ready
                sim.send(ClientMsg {
                    msg: Some(client_msg::Msg::StepReady(StepReady {
                        client_id,
                        tick_seq: 0,
                    })),
                })?;

                println!("[Client: {client_id}] waiting for tick from server...");
                // wait for the step to advance (block until Tick)
                loop {
                    match sim.recv()? {
                        ServerMsg {
                            msg: Some(server_msg::Msg::Tick(t)),
                        } => {
                            tick = t.seq;
                            println!("[Client: {client_id}] received tick from server: {}", tick);
                            break;
                        }
                        _ => { /* ignore other messages */ }
                    }
                }

                println!("[Client: {client_id}] doing work: {tick}");

                std::thread::sleep(Duration::from_millis(work_ms));
            }

            Ok(())
        })
        .expect("spawn controller thread")
}
