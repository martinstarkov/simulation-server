use anyhow::Result;
use bridge::server::Server;
use simulator::MySim;

fn main() -> Result<()> {
    let (_server, handle) = Server::new_with_grpc("127.0.0.1:50051", MySim::default());

    handle.join().unwrap()?;

    Ok(())
}
