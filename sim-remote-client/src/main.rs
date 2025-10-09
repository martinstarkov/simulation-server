use anyhow::Result;
use clap::Parser;
use sim_proto::pb::sim::{
    sim_msg::Kind,
    simulator_api_client::SimulatorApiClient,
    Command2,
    Empty,
    Heartbeat,
    // NEW:
    Register,
    SimMsg,
    StateAck,
    StepReady,
};
use std::time::Duration;
use tokio_stream::StreamExt;

#[derive(Parser)]
struct Args {
    /// Address of the simulator gRPC endpoint (remote service OR local app's rc port)
    #[arg(long, default_value = "127.0.0.1:50051")]
    addr: String,

    /// Optional command to send before subscribing (legacy, non-fenced)
    #[arg(long)]
    command: Option<String>,

    /// If set, this visualizer joins the step cohort and blocks stepping until it finishes rendering.
    #[arg(long, default_value_t = false)]
    blocking: bool,

    /// How many state messages to process before exiting
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

    // If blocking, register as a contributing client and start heartbeats
    if args.blocking {
        let _ = client
            .send(SimMsg {
                kind: Some(Kind::Register(Register {
                    app_id: args.app_id.clone(),
                    contributes: true,
                })),
            })
            .await?;

        // Heartbeat loop
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

    // Subscribe first so we don't miss early states
    let mut stream = client.subscribe(Empty {}).await?.into_inner();

    let mut count = 0usize;
    let mut last_tick_seen: u64 = 0;

    while count < args.n_states {
        match tokio::time::timeout(Duration::from_millis(5000), stream.next()).await {
            Ok(Some(Ok(msg))) => {
                if let Some(Kind::State(s)) = msg.kind {
                    // Only react to real physics steps: "state:<t>"
                    if let Some(t) = s.strip_prefix("state:").and_then(|n| n.parse::<u64>().ok()) {
                        if t > last_tick_seen {
                            last_tick_seen = t;

                            if args.blocking {
                                // 1) Ack that we received state t (required for cohort members)
                                let _ = client
                                    .send(SimMsg {
                                        kind: Some(Kind::Stateack(StateAck {
                                            app_id: args.app_id.clone(),
                                            tick: t,
                                        })),
                                    })
                                    .await?;

                                // 2) Issue *fenced* state retrieval
                                let _ = client
                                    .send(SimMsg {
                                        kind: Some(Kind::Cmd2(Command2 {
                                            app_id: args.app_id.clone(),
                                            fence: true, // block the barrier until handled
                                            cmd: format!("get-state:{t}"),
                                        })),
                                    })
                                    .await?;

                                // 3) "Render" with that state (simulate 1s render)
                                tokio::time::sleep(Duration::from_secs(1)).await;

                                // 4) Rendering finished: vote to step this tick
                                let _ = client
                                    .send(SimMsg {
                                        kind: Some(Kind::Stepready(StepReady {
                                            app_id: args.app_id.clone(),
                                            tick: t,
                                        })),
                                    })
                                    .await?;
                            } else {
                                // Non-blocking mode: not in cohort; just do a non-fenced retrieval
                                let _ = client
                                    .send(SimMsg {
                                        kind: Some(Kind::Cmd2(Command2 {
                                            app_id: args.app_id.clone(),
                                            fence: false,
                                            cmd: format!("get-state:{t}"),
                                        })),
                                    })
                                    .await?;

                                tokio::time::sleep(Duration::from_secs(1)).await;
                            }

                            println!("rendered scene");
                            count += 1;
                        }
                    }
                    // ignore "ack:*" and "retick:*"
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
        "[{}] processed {}/{} states; exiting.",
        args.app_id, count, args.n_states
    );
    Ok(())
}
