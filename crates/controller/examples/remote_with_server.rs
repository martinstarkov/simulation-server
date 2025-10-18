//! Two blocking controller threads against a gRPC server we start locally on an addr.
//! This exercises the remote (tonic) path while keeping everything in one process.

use std::{thread, time::Duration};

use anyhow::Result;
use bridge::client::Client;
use bridge::init_tracing;
use bridge::server::Server;
use controller::spawn_controller_thread;
use simulator::MySim;

fn main() -> Result<()> {
    init_tracing();

    let addr = "127.0.0.1:50051";

    let (server, h0) = Server::new_with_grpc(addr, MySim::default());

    thread::sleep(Duration::from_millis(100));

    let client_a = Client::new_remote(true, addr)?;
    let client_b = Client::new_remote(true, addr)?;

    let h1 = spawn_controller_thread(5, 40, client_a);
    let h2 = spawn_controller_thread(5, 600, client_b);

    h1.join().unwrap()?;
    h2.join().unwrap()?;

    server.shutdown();

    h0.join().unwrap()?;

    Ok(())
}
