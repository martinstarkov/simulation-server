mod common;

use anyhow::Result;
use bridge::init_tracing;
use bridge::{client::Client, server::Server};
use common::TestSim;
use std::thread;
use std::time::Duration;

#[test]
fn local_client_can_join_mid_simulation() -> Result<()> {
    init_tracing();

    let (server, sim_handle) = Server::new_local(TestSim::default());

    // Client A starts immediately
    let client_a = Client::new_local(true, &server)?;
    let h1 = thread::spawn({
        let client_a = client_a.clone();
        move || -> Result<()> {
            for _ in 0..8 {
                let tick = client_a.step_ready()?;
                println!("[client A] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(40));
            }
            Ok(())
        }
    });

    // Delay new client registration
    thread::sleep(Duration::from_millis(200));
    println!("[test] adding late client B");
    let client_b = Client::new_local(true, &server)?;
    let h2 = thread::spawn({
        let client_b = client_b.clone();
        move || -> Result<()> {
            for _ in 0..5 {
                let tick = client_b.step_ready()?;
                println!("[client B] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(30));
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

    println!("[test] local_client_can_join_mid_simulation finished cleanly");
    Ok(())
}
