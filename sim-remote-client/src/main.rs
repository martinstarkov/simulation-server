use anyhow::Result;
use clap::Parser;
use sim_proto::pb::sim::{sim_msg::Kind, simulator_api_client::SimulatorApiClient, Empty, SimMsg};
use std::time::Duration;
use tokio_stream::StreamExt;

#[derive(Parser)]
struct Args {
    /// Address of the simulator gRPC endpoint (remote service OR local app's rc port)
    #[arg(long, default_value = "127.0.0.1:50051")]
    addr: String,

    /// Optional command to send before subscribing
    #[arg(long)]
    command: Option<String>,

    /// How many state messages to print before exiting
    #[arg(long = "n-states", default_value_t = 5)]
    n_states: usize,

    /// Identifier printed alongside all client logs/states
    #[arg(long, default_value = "client-1")]
    app_id: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let mut client = SimulatorApiClient::connect(format!("http://{}", args.addr)).await?;
    println!("[{}] connected to {}", args.app_id, args.addr);

    if let Some(cmd) = &args.command {
        let _ = client
            .send(SimMsg {
                kind: Some(Kind::Command(format!("{cmd}-from-{}", args.app_id))),
            })
            .await?;
    }

    // Subscribe first so we don't miss early states
    let mut stream = client.subscribe(Empty {}).await?.into_inner();

    // Nudge simulator to produce activity
    let _ = client
        .send(SimMsg {
            kind: Some(Kind::Tick(1)),
        })
        .await?;

    let mut count = 0usize;
    while count < args.n_states {
        match tokio::time::timeout(Duration::from_secs(5), stream.next()).await {
            Ok(Some(Ok(msg))) => {
                if let Some(Kind::State(s)) = msg.kind {
                    println!("[{}] state: {}", args.app_id, s);
                    count += 1;
                }
            }
            Ok(Some(Err(status))) => {
                eprintln!("[{}] stream error: {}", args.app_id, status);
                break;
            }
            Ok(None) => {
                eprintln!("[{}] stream ended by server", args.app_id);
                break;
            }
            Err(_) => {
                eprintln!("[{}] timed out waiting for state", args.app_id);
                break;
            }
        }
    }

    println!(
        "[{}] printed {}/{} states; exiting.",
        args.app_id, count, args.n_states
    );
    Ok(())
}
