//! Generates TypeScript type definitions from Rust protocol types via ts-rs.
//!
//! Running `cargo test` produces `.ts` files in `web/src/lib/generated/`.
//! These files are committed to git so the frontend can import them directly.

#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live in tests/ directory, not inside #[cfg(test)] modules"
)]
mod ts_export {
    use ts_rs::TS;

    use residuum::gateway::protocol::{ClientMessage, ServerMessage};
    use residuum::models::ImageData;

    #[test]
    fn export_protocol_types() {
        let out_dir = "web/src/lib/generated";
        let cfg = ts_rs::Config::new().with_out_dir(out_dir);

        // Ensure the output directory exists
        std::fs::create_dir_all(out_dir).unwrap();

        // Export each type — dependencies are exported transitively
        ClientMessage::export_all(&cfg).unwrap();
        ServerMessage::export_all(&cfg).unwrap();
        ImageData::export_all(&cfg).unwrap();

        // Verify the generated files exist
        assert!(
            std::path::Path::new("web/src/lib/generated/ClientMessage.ts").exists(),
            "ClientMessage.ts should be generated"
        );
        assert!(
            std::path::Path::new("web/src/lib/generated/ServerMessage.ts").exists(),
            "ServerMessage.ts should be generated"
        );
        assert!(
            std::path::Path::new("web/src/lib/generated/ImageAttachment.ts").exists(),
            "ImageAttachment.ts should be generated (renamed from ImageData)"
        );
    }
}
