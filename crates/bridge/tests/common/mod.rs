use std::time::Duration;

use anyhow::Result;
use interface::{ClientMsg, ErrorMsg, ServerMsg, ServerMsgBody, Simulation, Tick};

#[derive(Default)]
pub struct TestSim {
    seq: u64,
    time_s: f32,
}

impl TestSim {
    fn dt(&self) -> f32 {
        1.0 / 60.0
    }
}

impl Simulation for TestSim {
    fn handle_message(&mut self, msg: ClientMsg) -> Result<Vec<ServerMsg>> {
        Ok(vec![ServerMsg {
            body: Some(ServerMsgBody::ErrorMsg(ErrorMsg {
                message: format!("Received from client {}", msg.client_id),
            })),
        }])
    }

    fn step(&mut self) -> Result<()> {
        self.seq += 1;
        self.time_s = self.seq as f32 * self.dt();
        println!("[sim] stepped to seq={} time={:.3}", self.seq, self.time_s);

        std::thread::sleep(Duration::from_secs_f32(self.dt()));

        Ok(())
    }

    fn get_tick(&self) -> Tick {
        Tick {
            seq: self.seq,
            time_s: self.time_s,
        }
    }
}
