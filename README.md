# Instructions:

1. Run `cargo build`

## Option 1: Local Client + Local Simulator:

Local Client + Simulator: 

`cargo run -p sim-app --bin app -- --mode local --n-states 100`

## Option 2: Local Client + Local Simulator + Remote Visualizer:

Local Client + Simulator: 

`cargo run -p sim-app --bin app -- --mode local --enable-rc --addr 127.0.0.1:60000 --n-states 100`

Remote Visualizer:

`cargo run -p sim-remote-client -- --addr 127.0.0.1:60000 --command "ping" --n-states 100`

## Option 3: Remote Simulator + Remote Client(s) + Remote Visualizer:

Remote Simulator:

`cargo run -p sim-app --bin sim_service -- --addr 127.0.0.1:50051`

Remote Client 1:

`cargo run -p sim-app --bin app -- --mode remote --addr 127.0.0.1:50051 --n-states 100 --app-id Client-1`

Remote Client 2:

`cargo run -p sim-app --bin app -- --mode remote --addr 127.0.0.1:50051 --n-states 100 --app-id Client-2`

Remote Visualizer:

`cargo run -p sim-remote-client -- --addr 127.0.0.1:50051 --command "ping" --n-states 100 --app-id visualizer-1`
