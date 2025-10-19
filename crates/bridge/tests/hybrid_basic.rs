//! Basic test: hybrid (local + remote) clients connected to gRPC server

mod common;

use anyhow::Result;
use bridge::{client::Client, init_tracing, server::Server};
use common::TestSim;
use std::{thread, time::Duration};

#[test]
fn hybrid_basic() -> Result<()> {
    init_tracing();

    let addr = "127.0.0.1:50051";

    // Start server with gRPC
    let (server, sim_handle) = Server::new_with_grpc(addr, TestSim::default());
    thread::sleep(Duration::from_millis(200)); // give gRPC a moment

    // One local, one remote
    let local_client = Client::new_local(true, &server)?;
    let remote_client = Client::new_remote(true, addr)?;

    let h_local = thread::spawn({
        let local_client = local_client.clone();
        move || -> Result<()> {
            for _ in 0..5 {
                let tick = local_client.step_ready()?;
                println!("[local] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(40));
            }
            Ok(())
        }
    });

    let h_remote = thread::spawn({
        let remote_client = remote_client.clone();
        move || -> Result<()> {
            for _ in 0..5 {
                let tick = remote_client.step_ready()?;
                println!("[remote] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(60));
            }
            Ok(())
        }
    });

    h_local.join().unwrap()?;
    h_remote.join().unwrap()?;

    // Shutdown cleanly
    server.shutdown();
    sim_handle.join().unwrap()?;

    println!("[test] hybrid basic finished cleanly");
    Ok(())
}
