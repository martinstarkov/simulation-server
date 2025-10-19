use std::thread;
use std::time::Duration;

use anyhow::Result;
use bridge::client::Client;

pub fn spawn_controller_thread(
    cycles: usize,
    work_ms: u64,
    sim: Client,
) -> thread::JoinHandle<Result<()>> {
    thread::spawn(move || -> Result<()> {
        for _i in 0..cycles {
            let tick = match sim.step_ready() {
                Ok(tick) => tick,
                Err(_) => {
                    break;
                }
            };

            // if i == 3 {
            //     sim.shutdown_server()?;
            // }

            println!("[client: {}] doing work: {}", sim.id(), tick.seq);

            std::thread::sleep(Duration::from_millis(work_ms));
        }

        Ok(())
    })
}
