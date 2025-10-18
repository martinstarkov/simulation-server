//! Test: remote client disconnects during simulation

mod common;

use anyhow::Result;
use bridge::{client::Client, init_tracing, server::Server};
use common::TestSim;
use std::{thread, time::Duration};

#[test]
fn remote_client_disconnects_mid_simulation() -> Result<()> {
    init_tracing();

    let addr = "127.0.0.1:50062";
    let (server, sim_handle) = Server::new_with_grpc(addr, TestSim::default());
    thread::sleep(Duration::from_millis(200));

    // Two remote clients
    let client_a = Client::new_remote(true, addr)?;
    let client_b = Client::new_remote(true, addr)?;

    let h1 = thread::spawn({
        let client_a = client_a.clone();
        move || -> Result<()> {
            for _ in 0..6 {
                let tick = match client_a.step_ready() {
                    Ok(t) => t,
                    Err(_) => break,
                };
                println!("[remote-A] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(40));
            }
            Ok(())
        }
    });

    let h2 = thread::spawn({
        let client_b = client_b.clone();
        move || -> Result<()> {
            for i in 0..8 {
                let tick = match client_b.step_ready() {
                    Ok(t) => t,
                    Err(_) => break,
                };
                println!("[remote-B] tick {}", tick.seq);
                if i == 3 {
                    println!("[remote-B] disconnecting intentionally");
                    drop(client_b); // 🔹 simulate abrupt disconnect
                    break;
                }
                thread::sleep(Duration::from_millis(60));
            }
            Ok(())
        }
    });

    h2.join().unwrap()?;

    drop(client_b);

    h1.join().unwrap()?;

    drop(client_a);

    server.shutdown();
    sim_handle.join().unwrap()?;

    println!("[test] remote_client_disconnects_mid_simulation finished cleanly");
    Ok(())
}
