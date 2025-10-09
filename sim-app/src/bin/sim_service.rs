use anyhow::Result;
use clap::Parser;
use std::net::SocketAddr;

#[derive(Parser)]
struct Args {
    /// Address to listen on (for app and remote clients)
    #[arg(long, default_value = "0.0.0.0:50051")]
    addr: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let addr: SocketAddr = args.addr.parse()?;
    sim_app::run_simulator_service(addr).await
}
