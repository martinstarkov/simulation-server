use anyhow::Result;
use clap::Parser;
use interface::ServerMode;
use server::create_server;
use std::thread;
use std::time::Duration;
use tracing::info;

/// Standalone simulator server.
///
/// By default it runs in gRPC mode listening on 127.0.0.1:50051.
/// Use `--local` to run a local-only in-proc coordinator (no gRPC).
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Run without gRPC networking (local-only mode).
    #[arg(long, default_value = "false")]
    local: bool,

    /// Listen address for gRPC mode (ignored if --local).
    #[arg(long, default_value = "127.0.0.1:50051")]
    addr: String,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<()> {
    // Setup tracing subscriber
    tracing_subscriber::fmt().with_env_filter("info").init();

    let args = Args::parse();

    if args.local {
        info!("Starting simulator in LOCAL-ONLY mode (no gRPC)");
        let _server = create_server(ServerMode::LocalOnly);
        // keep alive forever
        loop {
            thread::sleep(Duration::from_secs(3600));
        }
    } else {
        let addr = args.addr;
        info!("Starting simulator with gRPC on {addr}");
        let _server = create_server(ServerMode::WithGrpc(addr));
        // keep alive forever
        loop {
            thread::sleep(Duration::from_secs(3600));
        }
    }
}
