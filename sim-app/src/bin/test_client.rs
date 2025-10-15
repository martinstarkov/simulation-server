use sim_app::client::{SimClient, SimMode};
use sim_proto::pb::sim::{ClientMsg, ClientMsgBody, Register, StepReady};
use std::net::SocketAddr;
use std::time::Duration;

#[test]
fn test_local() -> anyhow::Result<()> {
    let client = SimClient::new(SimMode::Local, None)?;

    run_sim(client, "client-1", 10)?;
    Ok(())
}

#[test]
fn test_remote() -> anyhow::Result<()> {
    let addr: SocketAddr = "127.0.0.1:60000".parse()?;

    // Start a remote simulator service on another thread
    std::thread::spawn(move || {
        sim_app::run_simulator_service_blocking(addr).unwrap();
    });

    // Give it a moment to start
    std::thread::sleep(std::time::Duration::from_millis(300));

    let client = SimClient::new(SimMode::Remote, Some(addr))?;

    run_sim(client, "client-1", 10)?;
    Ok(())
}

//fn main() {}

fn run_sim(mut client: SimClient, app_id: &str, n_states: usize) -> anyhow::Result<()> {
    print!("Sending register message on client");
    client.send(ClientMsg {
        app_id: app_id.into(),
        body: Some(ClientMsgBody::Register(Register { contributes: true })),
    })?;

    client.send(ClientMsg {
        app_id: app_id.into(),
        body: Some(ClientMsgBody::StepReady(StepReady { tick: 0 })),
    })?;

    let mut processed = 0;
    let mut last_tick: u64 = 0;

    while processed < n_states {
        if let Some(msg) = client.recv() {
            if let Some(sim_proto::pb::sim::server_msg::Body::State(state)) = msg.body {
                if state.tick > last_tick {
                    last_tick = state.tick;
                    println!("tick {}", last_tick);
                    std::thread::sleep(Duration::from_millis(500));
                    client.send(ClientMsg {
                        app_id: app_id.into(),
                        body: Some(ClientMsgBody::StepReady(StepReady { tick: last_tick })),
                    })?;
                    processed += 1;
                }
            }
        }
    }

    println!("[{app_id}] finished after {processed} states");
    Ok(())
}

// #[tokio::test]
// async fn test_remote_client_connects() {
//     use std::net::SocketAddr;

//     let addr: SocketAddr = "127.0.0.1:60000".parse().unwrap();

//     // Run server on a background thread (blocking)
//     std::thread::spawn(move || {
//         sim_app::run_simulator_service_blocking(addr).unwrap();
//     });

//     // Wait a bit for server startup
//     tokio::time::sleep(std::time::Duration::from_millis(300)).await;

//     // Client uses synchronous interface
//     let client = SimClient::new(SimMode::Remote, addr).unwrap();

//     assert!(client.send(ClientMsg {
//         app_id: "test".into(),
//         body: None,
//     }).is_ok());
// }
