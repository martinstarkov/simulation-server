//! Basic test: local-only simulation with two clients

mod common;

use anyhow::Result;
use bridge::{client::Client, init_tracing, server::Server};
use common::TestSim;
use std::{thread, time::Duration};

#[test]
fn local_only_basic() -> Result<()> {
    init_tracing();

    // Start a local server (no gRPC)
    let (server, sim_handle) = Server::new_local(TestSim::default());

    // Two local clients
    let client_a = Client::new_local(true, &server)?;
    let client_b = Client::new_local(true, &server)?;

    // Each runs a few steps
    let h1 = thread::spawn({
        let client_a = client_a.clone();
        move || -> Result<()> {
            for _ in 0..5 {
                let tick = client_a.step_ready()?;
                println!("[local-A] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(20));
            }
            Ok(())
        }
    });

    let h2 = thread::spawn({
        let client_b = client_b.clone();
        move || -> Result<()> {
            for _ in 0..5 {
                let tick = client_b.step_ready()?;
                println!("[local-B] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(30));
            }
            Ok(())
        }
    });

    // Wait for both clients
    h1.join().unwrap()?;
    h2.join().unwrap()?;

    // Shutdown from main
    server.shutdown();
    sim_handle.join().unwrap()?;

    println!("[test] local-only basic finished cleanly");
    Ok(())
}
