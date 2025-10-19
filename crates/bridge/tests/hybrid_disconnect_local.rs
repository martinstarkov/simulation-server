//! Hybrid test: local client disconnects while remote client is still active.

mod common;

use anyhow::Result;
use bridge::{client::Client, init_tracing, server::Server};
use common::TestSim;
use std::{thread, time::Duration};

#[test]
fn hybrid_local_disconnect_mid_step() -> Result<()> {
    init_tracing();

    let addr = "127.0.0.1:50063";
    let (server, sim_handle) = Server::new_with_grpc(addr, TestSim::default());
    thread::sleep(Duration::from_millis(200));

    // Start both clients
    let local_client = Client::new_local(true, &server)?;
    let remote_client = Client::new_remote(true, addr)?;

    // Local client will disconnect mid-simulation
    let h_local = thread::spawn({
        let c = local_client.clone();
        move || -> Result<()> {
            for i in 0..8 {
                let tick = c.step_ready()?;
                println!("[local] tick {}", tick.seq);
                if i == 3 {
                    println!("[local] disconnecting intentionally");
                    drop(c); // simulate local drop
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
            Ok(())
        }
    });

    // Remote client keeps running
    let h_remote = thread::spawn({
        let c = remote_client.clone();
        move || -> Result<()> {
            for _ in 0..10 {
                match c.step_ready() {
                    Ok(tick) => {
                        println!("[remote] tick {}", tick.seq);
                        thread::sleep(Duration::from_millis(40));
                    }
                    Err(_) => {
                        println!("[remote] detected local disconnect or shutdown");
                        break;
                    }
                }
            }
            Ok(())
        }
    });

    h_local.join().unwrap()?;

    drop(local_client);

    h_remote.join().unwrap()?;

    drop(remote_client);

    server.shutdown();
    sim_handle.join().unwrap()?;

    println!("[test] hybrid_local_disconnect_mid_step finished cleanly");
    Ok(())
}
