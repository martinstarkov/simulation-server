use anyhow::Result;
use interface::{ClientMsgBody, ServerMsg};

pub trait Client {
    fn send(&self, msg: ClientMsgBody);
    fn recv(&self) -> Result<ServerMsg>;
    fn try_recv(&self) -> Option<ServerMsg>;
}
