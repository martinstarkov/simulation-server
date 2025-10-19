mod common;

use anyhow::Result;
use bridge::{client::Client, init_tracing, server::Server};
use common::TestSim;
use std::{thread, time::Duration};

#[test]
fn hybrid_local_late_join() -> Result<()> {
    init_tracing();

    let addr = "127.0.0.1:50100";
    let (server, sim_handle) = Server::new_with_grpc(addr, TestSim::default());
    thread::sleep(Duration::from_millis(200));

    // Start remote client first
    let remote_client = Client::new_remote(true, addr)?;
    let h_remote = thread::spawn({
        let c = remote_client.clone();
        move || -> Result<()> {
            for _ in 0..8 {
                let tick = c.step_ready()?;
                println!("[remote] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(50));
            }
            Ok(())
        }
    });

    // Local client joins mid-simulation
    thread::sleep(Duration::from_millis(250));
    println!("[test] adding late local client");
    let local_client = Client::new_local(true, &server)?;
    let h_local = thread::spawn({
        let c = local_client.clone();
        move || -> Result<()> {
            for _ in 0..5 {
                let tick = c.step_ready()?;
                println!("[local] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(40));
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

    println!("[test] hybrid_local_late_join finished cleanly");
    Ok(())
}
