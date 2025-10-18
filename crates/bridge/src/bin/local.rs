use bridge::client::Client;
use bridge::{local::LocalClient, server::ServerState};
use interface::{ActuatorCmd, ClientMsgBody, ControlOutput};

fn main() -> anyhow::Result<()> {
    // Create the server (no async runtime needed)
    let server = ServerState::new();

    // Start the synchronous simulation loop
    server.start_simulation();

    // Create two local clients
    let client_a = LocalClient::new(&server, true)?;
    let client_b = LocalClient::new(&server, false)?;

    // Send a test control message
    client_a.send(ClientMsgBody::Control(ControlOutput {
        actuators: vec![ActuatorCmd { id: 1, value: 0.75 }],
    }));

    // Read ticks
    for msg in client_b.iter() {
        println!("[local] client B got {:?}", msg.body);
    }

    Ok(())
}
