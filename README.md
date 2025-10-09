## Option 1: Local Client + Local Simulator:

Local Client + Local Simulator: 

`cargo run -p sim-app --bin app -- --mode local --n-states 1000`

## Option 2: Local Client + Local Simulator + Remote Visualizer:

Local Client + Local Simulator: 

`cargo run -p sim-app --bin app -- --mode local --remote-viewer --addr 127.0.0.1:60000 --n-states 1000`

Remote Visualizer:

Non-Blocking: `cargo run -p sim-remote-client -- --addr 127.0.0.1:60000 --n-states 1000 --app-id visualizer-1`
Blocking: `cargo run -p sim-remote-client -- --addr 127.0.0.1:60000 --n-states 1000 --app-id visualizer-1 --blocking`

## Option 3: Remote Simulator + Remote Client(s) + Remote Visualizer:

Remote Simulator:

`cargo run -p sim-app --bin sim_service -- --addr 127.0.0.1:50051`

Remote Client 1:

`cargo run -p sim-app --bin app -- --mode remote --addr 127.0.0.1:50051 --n-states 1000 --app-id Client-1`

Remote Client 2:

`cargo run -p sim-app --bin app -- --mode remote --addr 127.0.0.1:50051 --n-states 1000 --app-id Client-2`

Remote Visualizer:

Non-Blocking: `cargo run -p sim-remote-client -- --addr 127.0.0.1:50051 --n-states 1000 --app-id visualizer-1`
Blocking: `cargo run -p sim-remote-client -- --addr 127.0.0.1:50051 --n-states 1000 --app-id visualizer-1 --blocking`