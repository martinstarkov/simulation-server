pub mod pb {
    pub mod sim {
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/gen/sim.rs"));

        pub use client_msg::Body as ClientMsgBody;
        pub use client_msg::Body::*;
        pub use server_msg::Body as ServerMsgBody;
    }
}
