//! Test: remote client connects after simulation has already started.
mod common;

use anyhow::Result;
use bridge::{client::Client, init_tracing, server::Server};
use common::TestSim;
use std::{thread, time::Duration};

#[test]
fn remote_client_connects_late() -> Result<()> {
    init_tracing();

    let addr = "127.0.0.1:50061";
    let (server, sim_handle) = Server::new_with_grpc(addr, TestSim::default());

    // Let the simulation start running before remote clients connect
    thread::sleep(Duration::from_millis(200));

    println!("[test] connecting first remote client");
    let client_a = Client::new_remote(true, addr)?;

    let h1 = thread::spawn({
        let client_a = client_a.clone();
        move || -> Result<()> {
            for _ in 0..5 {
                let tick = client_a.step_ready()?;
                println!("[remote-A] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(60));
            }
            Ok(())
        }
    });

    // Connect second client later
    thread::sleep(Duration::from_millis(500));
    println!("[test] connecting second remote client mid-simulation");
    let client_b = Client::new_remote(true, addr)?;

    let h2 = thread::spawn({
        let client_b = client_b.clone();
        move || -> Result<()> {
            for _ in 0..4 {
                let tick = client_b.step_ready()?;
                println!("[remote-B] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(60));
            }
            Ok(())
        }
    });

    h1.join().unwrap()?;

    drop(client_a);

    h2.join().unwrap()?;

    drop(client_b);

    server.shutdown();
    sim_handle.join().unwrap()?;

    println!("[test] remote_client_connects_late finished cleanly");
    Ok(())
}
