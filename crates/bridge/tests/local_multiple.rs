mod common;

use anyhow::Result;
use bridge::init_tracing;
use bridge::{client::Client, server::Server};
use common::TestSim;
use std::{thread, time::Duration};

#[test]
fn two_local_clients_sync_barrier() -> Result<()> {
    init_tracing();

    let (server, sim_handle) = Server::new_local(TestSim::default());

    let client_a = Client::new_local(true, &server)?;
    let client_b = Client::new_local(true, &server)?;

    let h1 = thread::spawn({
        let client_a = client_a.clone();
        move || -> Result<()> {
            for _ in 0..5 {
                let tick = client_a.step_ready()?;
                println!("[client A] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(30));
            }
            Ok(())
        }
    });

    let h2 = thread::spawn({
        let client_b = client_b.clone();
        move || -> Result<()> {
            for _ in 0..5 {
                let tick = client_b.step_ready()?;
                println!("[client B] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(50));
            }
            Ok(())
        }
    });

    h1.join().unwrap()?;
    h2.join().unwrap()?;

    server.shutdown();
    sim_handle.join().unwrap()?;

    println!("[test] two_local_clients_sync_barrier finished cleanly");
    Ok(())
}
