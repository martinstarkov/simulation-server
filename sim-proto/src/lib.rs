pub mod pb {
    pub mod sim {
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/gen/sim.rs"));
    }
}
