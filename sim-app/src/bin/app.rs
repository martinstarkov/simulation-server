use anyhow::Result;
use clap::{Parser, ValueEnum};
use sim_app::{spawn_local, CONTROL_INTERVAL, STATE_WAIT_INTERVAL, STEP_INTERVAL};
use sim_proto::pb::sim::{
    client_msg::Body as CBody, server_msg::Body as SBody, ClientMsg, Register, ServerMsg, StepReady,
};
use std::net::SocketAddr;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

#[derive(ValueEnum, Clone)]
enum Mode {
    Local,
    Remote,
}

#[derive(Parser)]
struct Args {
    /// local = channels; remote = gRPC to remote simulator
    #[arg(long, value_enum, default_value = "local")]
    mode: Mode,

    /// Address of remote simulator service (for --mode remote)
    /// OR local viewer service port (when --remote-viewer)
    #[arg(long, default_value = "127.0.0.1:50051")]
    addr: String,

    /// When local, also expose a gRPC service so a remote viewer/client can connect.
    #[arg(long = "remote-viewer", default_value_t = false)]
    remote_viewer: bool,

    /// Identifier printed alongside all app logs/states
    #[arg(long, default_value = "app-1")]
    app_id: String,

    /// How many states to process before exiting
    #[arg(long = "n-states", default_value_t = 5)]
    n_states: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    match args.mode {
        Mode::Local => {
            run_local(
                args.remote_viewer,
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
    remote_viewer: bool,
    service_addr: SocketAddr,
    app_id: &str,
    n_states: usize,
) -> Result<()> {
    println!("[{app_id}] starting LOCAL simulator (channels)...");
    let (mut link, join) = spawn_local(remote_viewer, Some(service_addr)).await?;

    // Register as a contributing client
    link.send(ClientMsg {
        app_id: app_id.to_string(),
        body: Some(CBody::Register(Register { contributes: true })),
    })
    .await;

    // NEW: prime the barrier for the initial tick
    link.send(ClientMsg {
        app_id: app_id.to_string(),
        body: Some(CBody::StepReady(StepReady { tick: 0 })),
    })
    .await;

    let mut processed = 0usize;
    let mut last_tick: u64 = 0;

    while processed < n_states {
        if let Some(ServerMsg { body: Some(body) }) = link.next().await {
            if let SBody::State(state) = body {
                let t = state.tick;
                if t > last_tick {
                    last_tick = t;

                    tokio::time::sleep(CONTROL_INTERVAL).await;
                    println!("{t} thruster values found");

                    link.send(ClientMsg {
                        app_id: app_id.to_string(),
                        body: Some(CBody::StepReady(StepReady { tick: t })),
                    })
                    .await;

                    processed += 1;
                }
            }
        } else {
            break;
        }
    }

    println!("[{app_id}] local session done.");
    drop(link);
    let _ = join.await;
    Ok(())
}

async fn run_remote(addr: SocketAddr, app_id: &str, n_states: usize) -> Result<()> {
    use sim_proto::pb::sim::simulator_api_client::SimulatorApiClient;

    println!("[{app_id}] connecting to REMOTE simulator at {addr}...");
    let mut client = SimulatorApiClient::connect(format!("http://{addr}")).await?;

    // set up client->server stream
    let (tx_req, rx_req) = mpsc::channel::<ClientMsg>(128);
    let outbound = ReceiverStream::new(rx_req);

    // start bidi stream
    let mut rx = client.link(outbound).await?.into_inner();

    // Register as contributing
    tx_req
        .send(ClientMsg {
            app_id: app_id.into(),
            body: Some(CBody::Register(Register { contributes: true })),
        })
        .await?;

    // NEW: prime the barrier for the initial tick
    tx_req
        .send(ClientMsg {
            app_id: app_id.into(),
            body: Some(CBody::StepReady(StepReady { tick: 0 })),
        })
        .await?;

    let mut processed = 0usize;
    let mut last_tick: u64 = 0;

    while processed < n_states {
        match tokio::time::timeout(STATE_WAIT_INTERVAL, rx.next()).await {
            Ok(Some(Ok(ServerMsg {
                body: Some(SBody::State(s)),
            }))) => {
                let t = s.tick;
                if t > last_tick {
                    last_tick = t;

                    tokio::time::sleep(CONTROL_INTERVAL).await;
                    println!("{t} thruster values found");

                    tx_req
                        .send(ClientMsg {
                            app_id: app_id.into(),
                            body: Some(CBody::StepReady(StepReady { tick: t })),
                        })
                        .await?;

                    processed += 1;
                }
            }
            Ok(Some(Ok(_other))) => {}
            Ok(Some(Err(status))) => {
                eprintln!("[{app_id}] stream error: {status}");
                break;
            }
            Ok(None) => {
                eprintln!("[{app_id}] stream ended");
                break;
            }
            Err(_) => {
                eprintln!("[{app_id}] timed out waiting for state");
                break;
            }
        }
    }

    println!("[{app_id}] remote session done (processed {processed}/{n_states}).");
    Ok(())
}
