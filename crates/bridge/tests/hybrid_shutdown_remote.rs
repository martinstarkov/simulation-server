//! Hybrid test: remote client triggers shutdown, local detects closure.

mod common;

use anyhow::Result;
use bridge::{client::Client, init_tracing, server::Server};
use common::TestSim;
use std::{thread, time::Duration};

#[test]
fn hybrid_remote_shutdown() -> Result<()> {
    init_tracing();

    let addr = "127.0.0.1:50054";
    let (server, sim_handle) = Server::new_with_grpc(addr, TestSim::default());
    thread::sleep(Duration::from_millis(200));

    let local_client = Client::new_local(true, &server)?;
    let remote_client = Client::new_remote(true, addr)?;

    let h_remote = thread::spawn({
        let c = remote_client.clone();
        move || -> Result<()> {
            for i in 0..10 {
                let tick = match c.step_ready() {
                    Ok(t) => t,
                    Err(_) => break,
                };
                println!("[remote] tick {}", tick.seq);
                if i == 5 {
                    println!("[remote] sending shutdown_server()");
                    c.shutdown_server()?;
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
            Ok(())
        }
    });

    let h_local = thread::spawn({
        let c = local_client.clone();
        move || -> Result<()> {
            while let Ok(tick) = c.step_ready() {
                println!("[local] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(40));
            }
            println!("[local] server disconnected");
            Ok(())
        }
    });

    h_remote.join().unwrap()?;
    h_local.join().unwrap()?;
    sim_handle.join().unwrap()?;

    println!("[test] hybrid_remote_shutdown finished cleanly");
    Ok(())
}
