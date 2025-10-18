use bridge::{client::Client, remote::RemoteClient};
use interface::{ActuatorCmd, ClientMsgBody, ControlOutput};

fn main() -> anyhow::Result<()> {
    let addr = "127.0.0.1:50051";

    let client = RemoteClient::new(true, addr)?;

    client.send(ClientMsgBody::Control(ControlOutput {
        actuators: vec![ActuatorCmd { id: 1, value: 1.0 }],
    }));

    for msg in client.iter() {
        println!("[remote] got {:?}", msg.body);
    }

    Ok(())
}
