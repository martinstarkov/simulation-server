use anyhow::Result;
use clap::{Parser, ValueEnum};
use sim_app::{spawn_local, CONTROL_INTERVAL};
use sim_proto::pb::sim::{sim_msg::Kind, simulator_api_client::SimulatorApiClient, Empty, SimMsg};
use sim_proto::pb::sim::{Command2, Heartbeat, Register, StateAck, StepReady};
use std::{net::SocketAddr, time::Duration};
use tokio::sync::broadcast;
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
    /// OR remote-viewer gRPC port (for local mode with viewer)
    #[arg(long, default_value = "127.0.0.1:50051")]
    addr: String,

    /// When local, also expose a gRPC server so a remote viewer can connect.
    #[arg(long = "remote-viewer", default_value_t = false)]
    remote_viewer: bool,

    /// Identifier printed alongside all app logs/states
    #[arg(long, default_value = "app-1")]
    app_id: String,

    /// How many state messages the app should print before stopping
    #[arg(long = "n-states", default_value_t = 5)]
    n_states: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    match args.mode {
        Mode::Local => {
            run_local(
                args.remote_viewer, // <— renamed flag
                args.addr.parse()?, // used if remote_viewer = true
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
    remote_viewer: bool,
    rc_addr: SocketAddr,
    app_id: &str,
    n_states: usize,
) -> Result<()> {
    println!("[{app_id}] starting LOCAL simulator (channels)...");
    let (mut link, join) = spawn_local(remote_viewer, Some(rc_addr)).await?;
    // ^ when remote_viewer = true, this exposes the gRPC service for viewers
    //   but our local control client STILL participates via channels.

    // Register as a contributing client so the simulator waits for us
    link.send(SimMsg {
        kind: Some(Kind::Register(Register {
            app_id: app_id.to_string(),
            contributes: true,
        })),
    })
    .await;

    // Heartbeat to stay healthy for barrier membership
    {
        let tx = link.cmd_tx.clone();
        let id = app_id.to_string();
        tokio::spawn(async move {
            loop {
                let _ = tx
                    .send(SimMsg {
                        kind: Some(Kind::Heartbeat(Heartbeat { app_id: id.clone() })),
                    })
                    .await;
                tokio::time::sleep(Duration::from_millis(800)).await;
            }
        });
    }

    // Standard local control loop (unchanged behavior; always runs)
    let mut printed = 0usize;
    let mut last_tick_seen: u64 = 0;
    while printed < n_states {
        if let Some(msg) = link.next_state().await {
            if let Some(Kind::State(s)) = msg.kind {
                if let Some(t) = s.strip_prefix("state:").and_then(|n| n.parse::<u64>().ok()) {
                    if t > last_tick_seen {
                        last_tick_seen = t;

                        // ACK state t
                        link.send(SimMsg {
                            kind: Some(Kind::Stateack(StateAck {
                                app_id: app_id.to_string(),
                                tick: t,
                            })),
                        })
                        .await;

                        // Control work
                        tokio::time::sleep(CONTROL_INTERVAL).await;
                        println!("{t} thruster values found");

                        // Vote to step
                        link.send(SimMsg {
                            kind: Some(Kind::Stepready(StepReady {
                                app_id: app_id.to_string(),
                                tick: t,
                            })),
                        })
                        .await;

                        printed += 1;
                    }
                }
            }
        } else {
            break;
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

    // Register as a step-contributing client
    let _ = client
        .send(SimMsg {
            kind: Some(Kind::Register(Register {
                app_id: app_id.into(),
                contributes: true,
            })),
        })
        .await?;

    // Heartbeat task
    let mut hb_client = client.clone();
    let app = app_id.to_string();
    tokio::spawn(async move {
        loop {
            let _ = hb_client
                .send(SimMsg {
                    kind: Some(Kind::Heartbeat(Heartbeat {
                        app_id: app.clone(),
                    })),
                })
                .await;
            tokio::time::sleep(Duration::from_millis(800)).await;
        }
    });

    // Some initial noise (optional)
    let _ = client
        .send(SimMsg {
            kind: Some(Kind::Cmd2(Command2 {
                app_id: app_id.into(),
                fence: false,
                cmd: format!("init-from-{app_id}"),
            })),
        })
        .await?;
    // No manual Tick here; the server emits initial state:0.

    // Subscribe to state stream
    let mut stream = client.subscribe(Empty {}).await?.into_inner();

    // Helper to parse "state:<u64>" / "retick:<u64>"
    fn parse_tick(s: &str) -> Option<u64> {
        s.rsplit_once(':').and_then(|(_, n)| n.parse::<u64>().ok())
    }

    let mut count = 0usize;
    let mut last_tick_seen = 0u64;

    while count < n_states {
        match tokio::time::timeout(Duration::from_secs(5), stream.next()).await {
            Ok(Some(Ok(msg))) => {
                if let Some(Kind::State(s)) = msg.kind {
                    if let Some(t) = parse_tick(&s) {
                        if t > last_tick_seen {
                            last_tick_seen = t;

                            // ACK state t
                            let _ = client
                                .send(SimMsg {
                                    kind: Some(Kind::Stateack(StateAck {
                                        app_id: app_id.into(),
                                        tick: t,
                                    })),
                                })
                                .await;

                            // control loop work + log
                            tokio::time::sleep(CONTROL_INTERVAL).await;
                            println!("{t} thruster values found");

                            // vote to step
                            let _ = client
                                .send(SimMsg {
                                    kind: Some(Kind::Stepready(StepReady {
                                        app_id: app_id.into(),
                                        tick: t,
                                    })),
                                })
                                .await;

                            count += 1;
                        }
                    }
                    // else ignore ack:/retick:/other
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
                eprintln!("[{app_id}] timed out waiting for state (last_tick={last_tick_seen})");
                break;
            }
        }
    }

    println!("[{app_id}] remote session done (printed {count}/{n_states} states).");
    Ok(())
}
