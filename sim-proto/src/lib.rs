// No OUT_DIR needed; include from a fixed path produced by build.rs
pub mod pb {
    pub mod sim {
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/gen/sim.rs"));
    }
}
