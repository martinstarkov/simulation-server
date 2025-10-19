mod common;

use anyhow::Result;
use bridge::{client::Client, init_tracing, server::Server};
use common::TestSim;
use std::{thread, time::Duration};

#[test]
fn hybrid_multiple_locals_and_one_remote() -> Result<()> {
    init_tracing();

    let addr = "127.0.0.1:50103";
    let (server, sim_handle) = Server::new_with_grpc(addr, TestSim::default());
    thread::sleep(Duration::from_millis(200));

    // Two local clients
    let local_a = Client::new_local(true, &server)?;
    let local_b = Client::new_local(true, &server)?;
    // One remote
    let remote = Client::new_remote(true, addr)?;

    let h_a = thread::spawn({
        let c = local_a.clone();
        move || -> Result<()> {
            for _ in 0..6 {
                let tick = c.step_ready()?;
                println!("[local A] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(40));
            }
            Ok(())
        }
    });

    let h_b = thread::spawn({
        let c = local_b.clone();
        move || -> Result<()> {
            for _ in 0..6 {
                let tick = c.step_ready()?;
                println!("[local B] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(50));
            }
            Ok(())
        }
    });

    let h_r = thread::spawn({
        let c = remote.clone();
        move || -> Result<()> {
            for _ in 0..6 {
                let tick = c.step_ready()?;
                println!("[remote] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(60));
            }
            Ok(())
        }
    });

    h_a.join().unwrap()?;
    h_b.join().unwrap()?;
    h_r.join().unwrap()?;

    server.shutdown();
    sim_handle.join().unwrap()?;

    println!("[test] hybrid_multiple_locals_and_one_remote finished cleanly");
    Ok(())
}
