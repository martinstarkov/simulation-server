//! Two blocking controller threads against a local in-proc server (Coordinator + Simulation-based).

use anyhow::Result;
use bridge::client::Client;
use bridge::init_tracing;
use bridge::server::Server;
use controller::spawn_controller_thread;
use simulator::MySim;

fn main() -> Result<()> {
    init_tracing();

    let (server, h0) = Server::new_local(MySim::default());

    let client_a = Client::new_local(true, &server)?;
    let client_b = Client::new_local(true, &server)?;

    let h1 = spawn_controller_thread(5, 40, client_a);
    let h2 = spawn_controller_thread(5, 600, client_b);

    h1.join().unwrap()?;
    h2.join().unwrap()?;

    server.shutdown();

    h0.join().unwrap()?;

    Ok(())
}
