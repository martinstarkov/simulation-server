//! Basic test: server and remote clients only (no local clients)

mod common;

use anyhow::Result;
use bridge::{client::Client, init_tracing, server::Server};
use common::TestSim;
use std::{thread, time::Duration};

#[test]
fn remote_only_basic() -> Result<()> {
    init_tracing();

    let addr = "127.0.0.1:50052";

    // Start server
    let (server, sim_handle) = Server::new_with_grpc(addr, TestSim::default());
    thread::sleep(Duration::from_millis(200));

    // Two remote clients
    let c1 = Client::new_remote(true, addr)?;
    let c2 = Client::new_remote(true, addr)?;

    let h1 = thread::spawn({
        let c1 = c1.clone();
        move || -> Result<()> {
            for _ in 0..5 {
                let tick = c1.step_ready()?;
                println!("[remote-1] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(50));
            }
            Ok(())
        }
    });

    let h2 = thread::spawn({
        let c2 = c2.clone();
        move || -> Result<()> {
            for _ in 0..5 {
                let tick = c2.step_ready()?;
                println!("[remote-2] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(70));
            }
            Ok(())
        }
    });

    h1.join().unwrap()?;
    h2.join().unwrap()?;

    // Graceful shutdown
    server.shutdown();
    sim_handle.join().unwrap()?;

    println!("[test] remote-only basic finished cleanly");
    Ok(())
}
