use anyhow::Result;
use interface::{ClientMsg, ErrorMsg, ServerMsg, ServerMsgBody, Simulation};
use tracing::info;

#[derive(Default)]
pub struct MySim;

impl Simulation for MySim {
    fn handle_message(&mut self, msg: ClientMsg) -> Result<Vec<ServerMsg>> {
        Ok(vec![ServerMsg {
            body: Some(ServerMsgBody::ErrorMsg(ErrorMsg {
                message: "OK".into(),
            })),
        }])
    }

    fn dt(&self) -> f32 {
        1.0 / 60.0
    }
}
