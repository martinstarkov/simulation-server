use std::{default, sync::Arc};

use anyhow::Result;
use clap::Parser;
use sim_app::client::{Args, Client, ClientSync, Mode};
use sim_proto::pb::sim::{ClientMsg, ClientMsgBody, ServerMsgBody, StepReady};

fn main() -> Result<()> {
    let mut client = ClientSync::new("app-1")?;
    client.register()?;

    let mut last_tick = 0u64;

    for _ in 0..5 {
        client.run(|body| {
            match body {
                ServerMsgBody::State(state) => {
                    println!("tick {}", state.tick);
                    last_tick = state.tick;
                }
                _ => {}
            }
            Ok(())
        })?;

        client.ready(last_tick)?;
    }

    println!("done");
    Ok(())
}
