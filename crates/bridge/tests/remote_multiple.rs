mod common;

use anyhow::Result;
use bridge::{client::Client, init_tracing, server::Server};
use common::TestSim;
use std::{thread, time::Duration};

#[test]
fn remote_multiple_clients() -> Result<()> {
    init_tracing();

    // Start gRPC server
    let addr = "127.0.0.1:50110";
    let (server, sim_handle) = Server::new_with_grpc(addr, TestSim::default());

    // Give server a moment to start listening
    thread::sleep(Duration::from_millis(200));

    // Connect several remote clients
    let c1 = Client::new_remote(true, addr)?;
    let c2 = Client::new_remote(true, addr)?;
    let c3 = Client::new_remote(true, addr)?;

    // Spawn each client in its own thread
    let h1 = thread::spawn({
        let c = c1.clone();
        move || -> Result<()> {
            for _ in 0..6 {
                let tick = c.step_ready()?;
                println!("[remote-1] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(40));
            }
            Ok(())
        }
    });

    let h2 = thread::spawn({
        let c = c2.clone();
        move || -> Result<()> {
            for _ in 0..6 {
                let tick = c.step_ready()?;
                println!("[remote-2] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(50));
            }
            Ok(())
        }
    });

    let h3 = thread::spawn({
        let c = c3.clone();
        move || -> Result<()> {
            for i in 0..6 {
                let tick = c.step_ready()?;
                println!("[remote-3] tick {}", tick.seq);
                if i == 4 {
                    println!("[remote-3] disconnecting intentionally");
                    drop(c); // simulate remote drop
                    break;
                }
                thread::sleep(Duration::from_millis(60));
            }
            Ok(())
        }
    });

    h3.join().unwrap()?;
    drop(c3);
    h2.join().unwrap()?;
    drop(c2);
    h1.join().unwrap()?;
    drop(c1);

    // Gracefully shut down the server
    server.shutdown();
    sim_handle.join().unwrap()?;

    println!("[test] remote_multiple_clients finished cleanly");
    Ok(())
}
