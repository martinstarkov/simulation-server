use anyhow::Result;
use bridge::client::Client;
use crossbeam_channel as xchan;
use interface::{ServerMsg, ServerMsgBody};
use std::thread;

pub fn spawn_viewer_worker(sim: Client) -> (xchan::Receiver<ServerMsg>, xchan::Sender<()>) {
    let (tx_app, rx_app) = xchan::bounded::<ServerMsg>(1024);
    let (tx_ready, rx_ready) = xchan::bounded::<()>(1);

    thread::spawn(move || -> Result<()> {
        loop {
            if sim.step_participant() {
                let tick = sim.step_ready()?;

                tx_app.send(ServerMsg {
                    body: Some(ServerMsgBody::Tick(tick)),
                })?;

                // Block until Bevy signals "frame done"
                rx_ready.recv().ok();
            }

            sim.forward_all(&tx_app)?;
        }
    });

    (rx_app, tx_ready)
}
