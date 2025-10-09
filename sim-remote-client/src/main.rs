use anyhow::Result;
use clap::Parser;
use sim_proto::pb::sim::{
    client_msg::Body as CBody, server_msg::Body as SBody, simulator_api_client::SimulatorApiClient,
    ClientMsg, Register, Request, ServerMsg, StepReady,
};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

const STATE_WAIT_INTERVAL: Duration = Duration::from_millis(10000);
const RENDER_INTERVAL: Duration = Duration::from_millis(3000);

#[derive(Parser)]
struct Args {
    /// Address of the simulator gRPC endpoint (remote service OR local viewer port)
    #[arg(long, default_value = "127.0.0.1:50051")]
    addr: String,

    /// If set, this viewer participates in the step barrier (blocking).
    #[arg(long, default_value_t = false)]
    blocking: bool,

    /// How many ticks to print before exiting
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

    // Build client->server stream (outbound)
    let (tx_req, rx_req) = mpsc::channel::<ClientMsg>(128);
    let outbound = ReceiverStream::new(rx_req);

    // Start bidi stream: pass outbound, receive inbound
    let mut rx = client.link(outbound).await?.into_inner();

    // Register once (blocking or non-blocking)
    tx_req
        .send(ClientMsg {
            app_id: args.app_id.clone(),
            body: Some(CBody::Register(Register {
                contributes: args.blocking,
            })),
        })
        .await?;

    // Optional: send a sample data-bearing command
    tx_req
        .send(ClientMsg {
            app_id: args.app_id.clone(),
            body: Some(CBody::Request(Request {
                id: 1,
                name: "set-thrust".into(),
                data: vec![1, 2, 3, 4],
            })),
        })
        .await?;

    if args.blocking {
        // -------- BLOCKING MODE --------
        let mut count = 0usize;
        let mut last_tick_seen = 0u64;

        while count < args.n_states {
            match tokio::time::timeout(STATE_WAIT_INTERVAL, rx.next()).await {
                Ok(Some(Ok(ServerMsg { body: Some(body) }))) => match body {
                    SBody::State(s) => {
                        let t = s.tick;
                        if t <= last_tick_seen {
                            continue;
                        }
                        last_tick_seen = t;

                        tokio::time::sleep(RENDER_INTERVAL).await;

                        // do your work here if you like (no artificial sleep)
                        // then vote to advance
                        tx_req
                            .send(ClientMsg {
                                app_id: args.app_id.clone(),
                                body: Some(CBody::StepReady(StepReady { tick: t })),
                            })
                            .await?;

                        println!("{t} viewer (blocking)"); // prints every tick (cohort member)
                        count += 1;
                    }
                    SBody::Ack(a) => {
                        println!("ack id={} ok={} info='{}'", a.id, a.ok, a.info);
                    }
                    SBody::Response(_r) => { /* ignore for now */ }
                },
                Ok(Some(Ok(ServerMsg { body: None }))) => {
                    // ignore empty bodies
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
    } else {
        // -------- NON-BLOCKING MODE: always print the LATEST tick (skip older ticks) --------
        // Background task updates a watch channel with the latest tick as states arrive.
        let (latest_tx, mut latest_rx) = watch::channel::<u64>(0);

        tokio::spawn(async move {
            while let Some(item) = rx.next().await {
                match item {
                    Ok(ServerMsg {
                        body: Some(SBody::State(s)),
                    }) => {
                        let _ = latest_tx.send(s.tick); // only latest matters
                    }
                    Ok(ServerMsg {
                        body: Some(SBody::Ack(_)),
                    }) => { /* ignore */ }
                    Ok(ServerMsg {
                        body: Some(SBody::Response(_)),
                    }) => { /* ignore */ }
                    Ok(ServerMsg { body: None }) => { /* ignore */ }
                    Err(_status) => break, // stream error -> stop updating
                }
            }
        });

        // Wait for first tick so we don't print 0
        if tokio::time::timeout(STATE_WAIT_INTERVAL, latest_rx.changed())
            .await
            .is_err()
        {
            eprintln!("[{}] timed out waiting for first state", args.app_id);
            return Ok(());
        }

        let mut count = 0usize;
        let mut last_printed = 0u64;

        while count < args.n_states {
            // wait until latest changes (or already newer)
            if *latest_rx.borrow() <= last_printed {
                let _ = latest_rx.changed().await;
            }

            // snapshot the LATEST tick right before printing
            let t = *latest_rx.borrow();
            if t > last_printed {
                tokio::time::sleep(RENDER_INTERVAL).await;

                println!("{t} viewer (non-blocking current)"); // prints the current/latest tick; skips if we fell behind
                last_printed = t;
                count += 1;
            }
        }
    }

    println!("[{}] done.", args.app_id);
    Ok(())
}
