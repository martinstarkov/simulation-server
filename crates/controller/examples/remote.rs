//! Two blocking controller threads against a gRPC server we start locally on an addr.
//! This exercises the remote (tonic) path while keeping everything in one process.

use std::{thread, time::Duration};

use anyhow::Result;
use client::sync_client::connect_remote;
use controller::common::spawn_controller_thread;
use interface::ServerMode;
use server::{create_server, init_tracing};

fn main() -> Result<()> {
    init_tracing();

    let addr = "127.0.0.1:50051";

    let client = connect_remote(&addr)?;

    let h = spawn_controller_thread("ctrl-A", 500000, 40, client);

    h.join().unwrap()?;
    Ok(())
}
