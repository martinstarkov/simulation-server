//! Two blocking controller threads against a gRPC server we start locally on an addr.
//! This exercises the remote (tonic) path while keeping everything in one process.

use anyhow::Result;
use bridge::{client::Client, init_tracing};
use controller::spawn_controller_thread;

fn main() -> Result<()> {
    init_tracing();

    let addr = "127.0.0.1:50051";

    let client = Client::new_remote(true, addr)?;

    let h = spawn_controller_thread(500000, 40, client);

    h.join().unwrap()?;
    Ok(())
}
