use sim_app::{
    client::{new_client, Mode},
    init_tracing,
};

use crate::common::run_sim;

mod common;

#[test]
fn test_local() -> anyhow::Result<()> {
    init_tracing();

    let app_id = "client-1";

    let client = new_client(Mode::Local, true, app_id, None)?;

    run_sim(client, app_id, 5)?;

    Ok(())
}
