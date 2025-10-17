use anyhow::Result;
use interface::{ClientMsg, ErrorMsg, ServerMsg, ServerMsgBody, Simulation};
use tracing::info;

pub struct MySim;

impl Simulation for MySim {
    fn handle_message(&mut self, msg: ClientMsg) -> Result<Vec<ServerMsg>> {
        info!("[Server] Received message: {:?}", msg);
        Ok(vec![ServerMsg {
            body: Some(ServerMsgBody::ErrorMsg(ErrorMsg {
                message: "OK".into(),
            })),
        }])
    }
}
