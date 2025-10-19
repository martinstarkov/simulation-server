mod common;

use anyhow::Result;
use bridge::init_tracing;
use bridge::{client::Client, server::Server};
use common::TestSim;
use std::{thread, time::Duration};

#[test]
fn single_local_client_steps() -> Result<()> {
    init_tracing();

    let (server, sim_handle) = Server::new_local(TestSim::default());
    let client = Client::new_local(true, &server)?;

    for _ in 0..5 {
        let tick = client.step_ready()?;
        println!("[client] tick {}", tick.seq);
        thread::sleep(Duration::from_millis(20));
    }

    server.shutdown();
    sim_handle.join().unwrap()?;

    println!("[test] single_local_client_steps finished cleanly");
    Ok(())
}
