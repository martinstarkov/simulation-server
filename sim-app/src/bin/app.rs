use anyhow::Result;
use clap::{Parser, ValueEnum};
use sim_app::spawn_local;
use sim_proto::pb::sim::{sim_msg::Kind, simulator_api_client::SimulatorApiClient, Empty, SimMsg};
use std::{net::SocketAddr, time::Duration};
use tokio_stream::StreamExt;

#[derive(ValueEnum, Clone)]
enum Mode {
    Local,
    Remote,
}

#[derive(Parser)]
struct Args {
    /// local = channels, remote = gRPC
    #[arg(long, value_enum, default_value = "local")]
    mode: Mode,

    /// Address of remote simulator service (for --mode remote)
    /// OR remote-client port (for local rc)
    #[arg(long, default_value = "127.0.0.1:50051")]
    addr: String,

    /// When local, also expose a gRPC server so a remote client can connect.
    #[arg(long, default_value_t = false)]
    enable_rc: bool,

    /// Identifier printed alongside all app logs/states
    #[arg(long, default_value = "app-1")]
    app_id: String,

    /// How many state messages the app should print before stopping (or, in rc-mode, before idling)
    #[arg(long = "n-states", default_value_t = 5)]
    n_states: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    match args.mode {
        Mode::Local => {
            run_local(
                args.enable_rc,
                args.addr.parse()?,
                &args.app_id,
                args.n_states,
            )
            .await?
        }
        Mode::Remote => run_remote(args.addr.parse()?, &args.app_id, args.n_states).await?,
    }
    Ok(())
}

async fn run_local(
    enable_rc: bool,
    rc_addr: SocketAddr,
    app_id: &str,
    n_states: usize,
) -> Result<()> {
    println!("[{app_id}] starting LOCAL simulator (channels)...");
    let (mut link, join) = spawn_local(enable_rc, Some(rc_addr)).await?;

    // Kick some initial activity so subscribers see something
    link.send(SimMsg {
        kind: Some(Kind::Command(format!("init-from-{app_id}"))),
    })
    .await;
    link.send(SimMsg {
        kind: Some(Kind::Tick(0)),
    })
    .await;

    if enable_rc {
        println!("[{app_id}] remote-client gRPC listening at {rc_addr}");
        if n_states > 0 {
            // read N states on a small task, then stop reading; app stays alive for remote clients
            let mut rx = link.state_rx.resubscribe(); // take a receiver clone
            let id = app_id.to_string();
            tokio::spawn(async move {
                let mut count = 0usize;
                while count < n_states {
                    match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
                        Ok(Ok(msg)) => {
                            if let Some(Kind::State(s)) = msg.kind {
                                println!("[{id}] state: {s}");
                                count += 1;
                            }
                        }
                        Ok(Err(_closed)) => break, // channel gone
                        Err(_elapsed) => break,    // no states in window
                    }
                }
                println!("[{id}] printed {count}/{n_states} states; now idling for remote clients (Ctrl+C to stop).");
            });
        } else {
            println!("[{app_id}] not printing states (n-states=0); idling for remote clients.");
        }

        println!("[{app_id}] press Ctrl+C to stop.");
        tokio::signal::ctrl_c().await?;
        println!("\n[{app_id}] Ctrl+C received, shutting down...");
    } else {
        // No rc: just read N states and exit
        let mut printed = 0usize;
        while printed < n_states {
            if let Some(msg) = link.next_state().await {
                if let Some(Kind::State(s)) = msg.kind {
                    println!("[{app_id}] state: {s}");
                    printed += 1;
                }
            } else {
                break;
            }
        }
    }

    // graceful shutdown for the simulator core
    link.send(SimMsg {
        kind: Some(Kind::Shutdown(true)),
    })
    .await;
    let _ = join.await;
    println!("[{app_id}] local session done.");
    Ok(())
}

async fn run_remote(addr: SocketAddr, app_id: &str, n_states: usize) -> Result<()> {
    println!("[{app_id}] connecting to REMOTE simulator at {addr}...");
    let mut client = SimulatorApiClient::connect(format!("http://{addr}")).await?;

    let _ = client
        .send(SimMsg {
            kind: Some(Kind::Command(format!("init-from-{app_id}"))),
        })
        .await?;
    let _ = client
        .send(SimMsg {
            kind: Some(Kind::Tick(42)),
        })
        .await?;

    let mut stream = client.subscribe(Empty {}).await?.into_inner();
    let mut count = 0usize;
    while count < n_states {
        match tokio::time::timeout(Duration::from_secs(3), stream.next()).await {
            Ok(Some(Ok(msg))) => {
                if let Some(Kind::State(s)) = msg.kind {
                    println!("[{app_id}] state: {s}");
                    count += 1;
                }
            }
            Ok(Some(Err(status))) => {
                eprintln!("[{app_id}] stream error: {status}");
                break;
            }
            Ok(None) => {
                eprintln!("[{app_id}] stream ended");
                break;
            }
            Err(_elapsed) => {
                eprintln!("[{app_id}] timed out waiting for state");
                break;
            }
        }
    }

    println!("[{app_id}] remote session done (printed {count}/{n_states} states).");
    Ok(())
}
