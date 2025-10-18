use anyhow::Ok;
use bridge::{client::Client, local::LocalClient, remote::RemoteClient, server::ServerState};
use interface::{ActuatorCmd, ClientMsg, ClientMsgBody, ControlOutput};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create the shared server
    let server = ServerState::new();

    // Start synchronous simulation loop
    server.start_simulation();

    let addr = "127.0.0.1:50051";

    // Start gRPC endpoint in background
    {
        let srv = server.clone();
        tokio::spawn(async move {
            if let Err(e) = srv.start_grpc(addr).await {
                eprintln!("gRPC error: {:?}", e);
            }
        });
    }

    let local_client = LocalClient::new(&server, true)?;

    let client = RemoteClient::new(true, addr)?;

    client.send(ClientMsgBody::Control(ControlOutput {
        actuators: vec![ActuatorCmd { id: 1, value: 1.0 }],
    }));

    for msg in client.iter() {
        println!("[remote] got {:?}", msg.body);
    }

    for msg in local_client.iter() {
        println!("[local] got {:?}", msg.body);
    }

    Ok(())
}
