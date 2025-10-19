//! Hybrid test: remote client connects after simulation has already started.

mod common;

use anyhow::Result;
use bridge::{client::Client, init_tracing, server::Server};
use common::TestSim;
use std::{thread, time::Duration};

#[test]
fn hybrid_late_remote() -> Result<()> {
    init_tracing();

    let addr = "127.0.0.1:50052";
    let (server, sim_handle) = Server::new_with_grpc(addr, TestSim::default());
    thread::sleep(Duration::from_millis(200));

    let local_client = Client::new_local(true, &server)?;

    // Start local client first
    let h_local = thread::spawn({
        let c = local_client.clone();
        move || -> Result<()> {
            for _ in 0..10 {
                let tick = c.step_ready()?;
                println!("[local] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(50));
            }
            Ok(())
        }
    });

    // Wait before remote joins
    thread::sleep(Duration::from_millis(300));
    println!("[test] connecting remote client mid-simulation");
    let remote_client = Client::new_remote(true, addr)?;

    let h_remote = thread::spawn({
        let c = remote_client.clone();
        move || -> Result<()> {
            for _ in 0..5 {
                let tick = c.step_ready()?;
                println!("[remote] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(50));
            }
            Ok(())
        }
    });

    h_remote.join().unwrap()?;

    drop(remote_client);

    h_local.join().unwrap()?;

    drop(local_client);

    server.shutdown();
    sim_handle.join().unwrap()?;

    println!("[test] hybrid_late_remote finished cleanly");
    Ok(())
}
