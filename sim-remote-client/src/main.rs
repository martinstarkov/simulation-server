use anyhow::Result;
use clap::Parser;
use sim_proto::pb::sim::{
    sim_msg::Kind, simulator_api_client::SimulatorApiClient, Command2, Empty, Heartbeat, Register,
    SimMsg, StateAck, StepReady,
};
use std::time::Duration;
use tokio_stream::StreamExt;

const RENDER_INTERVAL: Duration = Duration::from_millis(2000);
const FIRST_STATE_TIMEOUT: Duration = Duration::from_millis(5000);

#[derive(Parser)]
struct Args {
    /// Address of the simulator gRPC endpoint (remote service OR local app's viewer port)
    #[arg(long, default_value = "127.0.0.1:50051")]
    addr: String,

    /// Optional command to send before subscribing (legacy, non-fenced)
    #[arg(long)]
    command: Option<String>,

    /// If set, this visualizer joins the step cohort and blocks stepping until it finishes rendering.
    #[arg(long, default_value_t = false)]
    blocking: bool,

    /// How many states/renders before exiting
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
                kind: Some(Kind::Command(cmd.clone())),
            })
            .await?;
    }

    // Helper: parse "state:<u64>"
    fn parse_tick(s: &str) -> Option<u64> {
        s.strip_prefix("state:").and_then(|n| n.parse::<u64>().ok())
    }

    // BLOCKING VIEWER: inline stream handling (participates in barrier)
    if args.blocking {
        let _ = client
            .send(SimMsg {
                kind: Some(Kind::Register(Register {
                    app_id: args.app_id.clone(),
                    contributes: true,
                })),
            })
            .await?;

        // Heartbeats
        {
            let mut hb_client = client.clone();
            let hb_id = args.app_id.clone();
            tokio::spawn(async move {
                loop {
                    let _ = hb_client
                        .send(SimMsg {
                            kind: Some(Kind::Heartbeat(Heartbeat {
                                app_id: hb_id.clone(),
                            })),
                        })
                        .await;
                    tokio::time::sleep(Duration::from_millis(800)).await;
                }
            });
        }

        // Subscribe and process states inline
        let mut stream = client.subscribe(Empty {}).await?.into_inner();

        let mut count = 0usize;
        let mut last_tick_seen: u64 = 0;

        while count < args.n_states {
            match tokio::time::timeout(FIRST_STATE_TIMEOUT, stream.next()).await {
                Ok(Some(Ok(msg))) => {
                    if let Some(Kind::State(s)) = msg.kind {
                        if let Some(t) = parse_tick(&s) {
                            if t > last_tick_seen {
                                last_tick_seen = t;

                                // 1) Ack state t
                                let _ = client
                                    .send(SimMsg {
                                        kind: Some(Kind::Stateack(StateAck {
                                            app_id: args.app_id.clone(),
                                            tick: t,
                                        })),
                                    })
                                    .await?;

                                // 2) Fenced fetch for t (blocks barrier until drained)
                                let _ = client
                                    .send(SimMsg {
                                        kind: Some(Kind::Cmd2(Command2 {
                                            app_id: args.app_id.clone(),
                                            fence: true,
                                            cmd: format!("get-state:{t}"),
                                        })),
                                    })
                                    .await?;

                                // 3) Render
                                tokio::time::sleep(RENDER_INTERVAL).await;

                                // 4) Vote to step
                                let _ = client
                                    .send(SimMsg {
                                        kind: Some(Kind::Stepready(StepReady {
                                            app_id: args.app_id.clone(),
                                            tick: t,
                                        })),
                                    })
                                    .await?;

                                println!("{t} rendered scene (blocking)");
                                count += 1;
                            }
                        }
                    }
                    // ignore "ack:*", "retick:*", "lagged", etc.
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
            "[{}] processed {}/{} states; exiting.",
            args.app_id, count, args.n_states
        );
        return Ok(());
    }

    // NON-BLOCKING VIEWER: background subscriber updates the latest tick; render loop snapshots it.
    let mut stream = client.subscribe(Empty {}).await?.into_inner();

    // A watch channel is perfect to publish "latest tick" snapshots.
    let (latest_tx, mut latest_rx) = tokio::sync::watch::channel::<u64>(0);

    // Spawn background task that *only* updates latest tick as states arrive.
    tokio::spawn(async move {
        let mut latest = 0u64;
        while let Some(next) = stream.next().await {
            match next {
                Ok(msg) => {
                    if let Some(Kind::State(s)) = msg.kind {
                        if let Some(t) = parse_tick(&s) {
                            if t > latest {
                                latest = t;
                                // ignore send errors (no receivers) – then we can just exit
                                let _ = latest_tx.send(latest);
                            }
                        }
                    }
                }
                Err(_status) => {
                    // stream error; just break and let main loop handle lack of updates
                    break;
                }
            }
        }
    });

    // Render loop: each iteration uses the *latest tick value at that moment* (may skip frames)
    let mut count = 0usize;
    let mut last_rendered_tick = 0u64;

    // Wait for the first tick to show up within timeout
    let first_ok = tokio::time::timeout(FIRST_STATE_TIMEOUT, latest_rx.changed()).await;
    if first_ok.is_err() {
        eprintln!("[{}] timed out waiting for first state", args.app_id);
        return Ok(());
    }

    while count < args.n_states {
        // Block until a new/latest value is published that surpasses what we last rendered
        // If no new value arrives for a while, we’ll just proceed to render the same tick again?
        // Usually you want a *new* tick; so wait until it changes or time out and continue.
        let mut snapshot: u64 = *latest_rx.borrow();
        if snapshot <= last_rendered_tick {
            // wait for a change
            let _ = latest_rx.changed().await;
            snapshot = *latest_rx.borrow();
        }

        // Snapshot taken RIGHT BEFORE rendering -> exactly what you asked.
        let t = snapshot;

        // Request state for that specific t (non-fenced; we are a viewer)
        let _ = client
            .send(SimMsg {
                kind: Some(Kind::Cmd2(Command2 {
                    app_id: args.app_id.clone(),
                    fence: false,
                    cmd: format!("get-state:{t}"),
                })),
            })
            .await?;

        // Simulate rendering time
        tokio::time::sleep(RENDER_INTERVAL).await;

        let skipped = if last_rendered_tick > 0 && t > last_rendered_tick {
            t.saturating_sub(last_rendered_tick + 1)
        } else {
            0
        };
        if skipped > 0 {
            println!(
                "{t} rendered scene (non-blocking, skipped {skipped} frame{})",
                if skipped == 1 { "" } else { "s" }
            );
        } else {
            println!("{t} rendered scene (non-blocking)");
        }

        last_rendered_tick = t;
        count += 1;
    }

    println!(
        "[{}] processed {}/{} states; exiting.",
        args.app_id, count, args.n_states
    );
    Ok(())
}
