use visualizer::{VisualizerMode, run};

fn main() {
    run(VisualizerMode::LocalBlockingWithServer).expect("visualizer exited");
}
