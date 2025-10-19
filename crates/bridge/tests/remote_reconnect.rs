//! Test: remote client disconnects and reconnects later

mod common;

use anyhow::Result;
use bridge::{client::Client, init_tracing, server::Server};
use common::TestSim;
use std::{thread, time::Duration};

#[test]
fn remote_client_reconnects() -> Result<()> {
    init_tracing();

    let addr = "127.0.0.1:50063";
    let (server, sim_handle) = Server::new_with_grpc(addr, TestSim::default());
    thread::sleep(Duration::from_millis(200));

    // First connection
    println!("[test] connecting remote client");
    let client = Client::new_remote(true, addr)?;
    for _ in 0..3 {
        let tick = client.step_ready()?;
        println!("[remote-client] tick {}", tick.seq);
        thread::sleep(Duration::from_millis(50));
    }

    // Simulate disconnect (drop client)
    println!("[test] simulating remote disconnect");
    drop(client);
    thread::sleep(Duration::from_millis(200));

    // Reconnect again
    println!("[test] reconnecting remote client");
    let client2 = Client::new_remote(true, addr)?;
    for _ in 0..3 {
        let tick = client2.step_ready()?;
        println!("[remote-client(reconnect)] tick {}", tick.seq);
        thread::sleep(Duration::from_millis(50));
    }

    server.shutdown();
    sim_handle.join().unwrap()?;

    println!("[test] remote_client_reconnects finished cleanly");
    Ok(())
}
