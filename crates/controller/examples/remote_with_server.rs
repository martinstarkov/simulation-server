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

    let addr = "127.0.0.1:50091";

    let _server = create_server(ServerMode::WithGrpc(addr.to_string()));

    thread::sleep(Duration::from_millis(100));

    let client_a = connect_remote(&addr)?;
    let client_b = connect_remote(&addr)?;

    let h1 = spawn_controller_thread("ctrl-A", 5, 40, client_a);

    let h2 = spawn_controller_thread("ctrl-B", 5, 600, client_b);

    h1.join().unwrap()?;
    h2.join().unwrap()?;
    Ok(())
}
