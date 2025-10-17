//! Two blocking controller threads against a local in-proc server (Coordinator + Simulation-based).

use anyhow::Result;
use client::client::connect_local;
use controller::common::spawn_controller_thread;
use server::{create_local_server, init_tracing};

fn main() -> Result<()> {
    init_tracing();

    let server = create_local_server();

    let client_a = connect_local(true, &server)?;
    let client_b = connect_local(true, &server)?;

    let h1 = spawn_controller_thread(5, 40, client_a);
    let h2 = spawn_controller_thread(5, 600, client_b);

    h1.join().unwrap()?;
    h2.join().unwrap()?;
    Ok(())
}
