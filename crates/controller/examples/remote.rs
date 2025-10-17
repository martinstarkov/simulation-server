//! Two blocking controller threads against a gRPC server we start locally on an addr.
//! This exercises the remote (tonic) path while keeping everything in one process.

use anyhow::Result;
use client::client::connect_remote;
use controller::common::spawn_controller_thread;
use server::init_tracing;

fn main() -> Result<()> {
    init_tracing();

    let addr = "127.0.0.1:50051";

    let client = connect_remote(true, &addr)?;

    let h = spawn_controller_thread(500000, 40, client);

    h.join().unwrap()?;
    Ok(())
}
