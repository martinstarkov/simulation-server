//! Hybrid test: one local and two remote clients step together.

mod common;

use anyhow::Result;
use bridge::{client::Client, init_tracing, server::Server};
use common::TestSim;
use std::{thread, time::Duration};

#[test]
fn hybrid_multiple_remotes() -> Result<()> {
    init_tracing();

    let addr = "127.0.0.1:50055";
    let (server, sim_handle) = Server::new_with_grpc(addr, TestSim::default());
    thread::sleep(Duration::from_millis(200));

    let local_client = Client::new_local(true, &server)?;
    let r1 = Client::new_remote(true, addr)?;
    let r2 = Client::new_remote(true, addr)?;

    let h_local = thread::spawn({
        let c = local_client.clone();
        move || -> Result<()> {
            for _ in 0..6 {
                let tick = c.step_ready()?;
                println!("[local] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(40));
            }
            Ok(())
        }
    });

    let h_r1 = thread::spawn({
        let c = r1.clone();
        move || -> Result<()> {
            for _ in 0..6 {
                let tick = c.step_ready()?;
                println!("[remote-1] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(50));
            }
            Ok(())
        }
    });

    let h_r2 = thread::spawn({
        let c = r2.clone();
        move || -> Result<()> {
            for _ in 0..6 {
                let tick = c.step_ready()?;
                println!("[remote-2] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(60));
            }
            Ok(())
        }
    });

    h_local.join().unwrap()?;
    h_r1.join().unwrap()?;
    h_r2.join().unwrap()?;

    server.shutdown();
    sim_handle.join().unwrap()?;

    println!("[test] hybrid_multiple_remotes finished cleanly");
    Ok(())
}
