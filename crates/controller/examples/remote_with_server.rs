//! Two blocking controller threads against a gRPC server we start locally on an addr.
//! This exercises the remote (tonic) path while keeping everything in one process.

use std::{thread, time::Duration};

use anyhow::Result;
use client::client::connect_remote;
use controller::common::spawn_controller_thread;
use server::{create_remote_server, init_tracing};

fn main() -> Result<()> {
    init_tracing();

    let addr = "127.0.0.1:50051";

    let _server = create_remote_server(&addr);

    thread::sleep(Duration::from_millis(100));

    let client_a = connect_remote(true, &addr)?;
    let client_b = connect_remote(true, &addr)?;

    let h1 = spawn_controller_thread(5, 40, client_a);
    let h2 = spawn_controller_thread(5, 600, client_b);

    h1.join().unwrap()?;
    h2.join().unwrap()?;
    Ok(())
}
