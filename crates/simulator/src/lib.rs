use anyhow::Result;
use interface::{
    Simulation,
    interface::{ClientMsg, ServerMsg, server_msg},
};
use tracing::info;

pub struct MySim;

impl Simulation for MySim {
    fn handle_message(&mut self, msg: ClientMsg) -> Result<Vec<ServerMsg>> {
        info!("Server received message: {:?}", msg);
        Ok(vec![ServerMsg {
            msg: Some(server_msg::Msg::ErrorMsg(interface::interface::ErrorMsg {
                message: "OK".into(),
            })),
        }])
    }
}
