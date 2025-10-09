# Instructions:

1. Run `cargo build`

## Option 1: Local Client + Local Simulator:

Local Client + Local Simulator: 

`cargo run -p sim-app --bin app -- --mode local --n-states 100`

Note: We set freerun=true and never add the local app to the step cohort. The core free-ticks every STEP_INTERVAL with no waiting.

## Option 2: Local Client + Local Simulator + Remote Visualizer:

Local Client + Local Simulator: 

`cargo run -p sim-app --bin app -- --mode local --enable-rc --addr 127.0.0.1:60000 --n-states 100`

Remote Visualizer:

Non-Blocking: `cargo run -p sim-remote-client -- --addr 127.0.0.1:60000 --n-states 100 --app-id visualizer-1`
Blocking: `cargo run -p sim-remote-client -- --addr 127.0.0.1:60000 --n-states 100 --app-id visualizer-1 --blocking`

Note: Still freerun=true. If a remote client sends cmd2(fence=true) -> it enters the blocking queue and the next barrier cycle drains it before stepping. Remote client registers with contributes=true -> it is added to the cohort; the server now waits for its StepReady(t) and StateAck(t), evicting it on timeout/heartbeat loss so locals don’t stall forever.


## Option 3: Remote Simulator + Remote Client(s) + Remote Visualizer:

Remote Simulator:

`cargo run -p sim-app --bin sim_service -- --addr 127.0.0.1:50051`

Remote Client 1:

`cargo run -p sim-app --bin app -- --mode remote --addr 127.0.0.1:50051 --n-states 100 --app-id Client-1`

Remote Client 2:

`cargo run -p sim-app --bin app -- --mode remote --addr 127.0.0.1:50051 --n-states 100 --app-id Client-2`

Remote Visualizer:

Non-Blocking: `cargo run -p sim-remote-client -- --addr 127.0.0.1:50051 --n-states 100 --app-id visualizer-1`
Blocking: `cargo run -p sim-remote-client -- --addr 127.0.0.1:50051 --n-states 100 --app-id visualizer-1 --blocking`

Note: freerun=false -> always uses the barrier logic. Clients/ visualizers can participate: Visualizer may set contributes=false but still send fenced queries/mutations; those pause the barrier until responded. Clients set contributes=true and must drive StateAck/StepReady as shown.