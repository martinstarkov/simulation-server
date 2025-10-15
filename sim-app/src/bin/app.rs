use anyhow::Result;
use clap::{Parser, ValueEnum};
use sim_app::{spawn_hybrid, spawn_local, LocalAppLink, CONTROL_INTERVAL, STATE_WAIT_INTERVAL};
use sim_proto::pb::sim::{ClientMsg, ClientMsgBody, Register, ServerMsg, ServerMsgBody, StepReady};
use std::net::SocketAddr;
use std::thread;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;
use tracing::info;

#[derive(ValueEnum, Clone)]
enum Mode {
    Local,
    Hybrid,
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

    /// Identifier alongside all app logs/states
    #[arg(long, default_value = "app-1")]
    app_id: String,

    /// How many states to process before exiting
    #[arg(long = "n-states", default_value_t = 5)]
    n_states: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let id = &args.app_id;

    match args.mode {
        Mode::Local => {
            let (link, join) = spawn_local(id)?;
            run_local(link, id, args.n_states)?;
            join.join().expect("Simulator thread crashed");
        }
        Mode::Hybrid => {
            let addr = args.addr.parse()?;
            let (link, core_join, svc_join, _svc_shutdown) = spawn_hybrid(id, addr)?;
            run_local(link, id, args.n_states)?;
            core_join.join().expect("Simulator thread crashed");
            svc_join.join().expect("Service thread crashed");
        }
        Mode::Remote => {
            let addr = args.addr.parse()?;
            run_remote(addr, id, args.n_states).await?
        }
    }
    Ok(())
}

fn run_local(link: LocalAppLink, app_id: &str, n_states: usize) -> Result<()> {
    // Register as a contributing client.
    link.send(ClientMsg {
        app_id: app_id.to_string(),
        body: Some(ClientMsgBody::Register(Register { contributes: true })),
    });

    // Prime the barrier for the initial tick.
    link.send(ClientMsg {
        app_id: app_id.to_string(),
        body: Some(ClientMsgBody::StepReady(StepReady { tick: 0 })),
    });

    let mut processed = 0usize;
    let mut last_tick: u64 = 0;

    while processed < n_states {
        if let Some(ServerMsg { body: Some(body) }) = link.next() {
            if let ServerMsgBody::State(state) = body {
                let t = state.tick;
                if t > last_tick {
                    last_tick = t;

                    thread::sleep(CONTROL_INTERVAL);

                    info!("{t} thruster values found");

                    link.send(ClientMsg {
                        app_id: app_id.to_string(),
                        body: Some(ClientMsgBody::StepReady(StepReady { tick: t })),
                    });

                    processed += 1;
                }
            }
        } else {
            break;
        }
    }

    info!("[{app_id}] local session done.");
    drop(link);

    Ok(())
}

async fn run_remote(addr: SocketAddr, app_id: &str, n_states: usize) -> Result<()> {
    use sim_proto::pb::sim::simulator_api_client::SimulatorApiClient;
    let mut client = SimulatorApiClient::connect(format!("http://{addr}")).await?;

    // set up client->server stream
    let (tx_req, rx_req) = mpsc::channel::<ClientMsg>(128);
    let outbound = ReceiverStream::new(rx_req);

    // start bidi stream
    let mut rx = client.link(outbound).await?.into_inner();

    // Register as contributing client.
    tx_req
        .send(ClientMsg {
            app_id: app_id.into(),
            body: Some(ClientMsgBody::Register(Register { contributes: true })),
        })
        .await?;

    // Prime the barrier for the initial tick.
    tx_req
        .send(ClientMsg {
            app_id: app_id.into(),
            body: Some(ClientMsgBody::StepReady(StepReady { tick: 0 })),
        })
        .await?;

    let mut processed = 0usize;
    let mut last_tick: u64 = 0;

    while processed < n_states {
        match tokio::time::timeout(STATE_WAIT_INTERVAL, rx.next()).await {
            Ok(Some(Ok(ServerMsg {
                body: Some(ServerMsgBody::State(s)),
            }))) => {
                let t = s.tick;
                if t > last_tick {
                    last_tick = t;

                    tokio::time::sleep(CONTROL_INTERVAL).await;
                    info!("{t} thruster values found");

                    tx_req
                        .send(ClientMsg {
                            app_id: app_id.into(),
                            body: Some(ClientMsgBody::StepReady(StepReady { tick: t })),
                        })
                        .await?;

                    processed += 1;
                }
            }
            Ok(Some(Ok(_other))) => {}
            Ok(Some(Err(status))) => {
                info!("[{app_id}] stream error: {status}");
                break;
            }
            Ok(None) => {
                info!("[{app_id}] stream ended");
                break;
            }
            Err(_) => {
                info!("[{app_id}] timed out waiting for state");
                break;
            }
        }
    }

    info!("[{app_id}] remote session done (processed {processed}/{n_states}).");
    Ok(())
}
