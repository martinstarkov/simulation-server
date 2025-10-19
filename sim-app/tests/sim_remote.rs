use sim_app::{
    client::{new_client, Mode},
    init_tracing,
};
use std::net::SocketAddr;

use crate::common::run_sim;

mod common;

#[test]
fn test_remote() -> anyhow::Result<()> {
    init_tracing();

    let addr: SocketAddr = "127.0.0.1:60000".parse()?;

    // Start a remote simulator service on another thread.
    std::thread::spawn(move || {
        sim_app::run_simulator_service_blocking(addr).unwrap();
    });

    // Give it a moment to start.
    std::thread::sleep(std::time::Duration::from_millis(300));

    let app_id = "client-1";

    let client = new_client(Mode::Remote, true, app_id, Some(addr))?;

    run_sim(client, app_id, 5)?;
    Ok(())
}
