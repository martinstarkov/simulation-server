//! Two blocking controller threads against a local in-proc server (Coordinator + Simulation-based).

use anyhow::Result;
use client::sync_client::connect_local;
use controller::common::spawn_controller_thread;
use interface::ServerMode;
use server::{create_server, init_tracing};
use std::sync::Arc; // your simulation type

fn main() -> Result<()> {
    init_tracing();

    let server = Arc::new(create_server(ServerMode::LocalOnly));

    let client_a = connect_local(&Arc::clone(&server))?;
    let client_b = connect_local(&Arc::clone(&server))?;

    let h1 = spawn_controller_thread("ctrl-A", 5, 40, client_a);

    let h2 = spawn_controller_thread("ctrl-B", 5, 600, client_b);

    h1.join().unwrap()?;
    h2.join().unwrap()?;
    Ok(())
}
