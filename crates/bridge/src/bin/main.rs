use bridge::server::ServerState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let addr = "127.0.0.1:50051";

    // Create the server
    let server = ServerState::new();

    // Start the simulation loop
    server.start_simulation();

    // Only gRPC service — no local clients
    server.start_grpc(addr).await?;

    Ok(())
}
