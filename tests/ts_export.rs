//! Generates TypeScript type definitions from Rust protocol types via ts-rs.
//!
//! Running `cargo test` produces `.ts` files in `web/src/lib/generated/`.
//! These files are committed to git so the frontend can import them directly.

#[expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live in tests/ directory, not inside #[cfg(test)] modules"
)]
mod ts_export {
    use ts_rs::TS;

    use residuum::checkpoints::{
        CheckpointDetail, CheckpointPage, RepoKind, RepoStats, RestoreOutcome, UndoOutcome,
    };
    use residuum::gateway::protocol::{
        ActionInfo, ArtifactSummary, ClientMessage, PulseInfo, ServerMessage, SessionListResponse,
        WorkbenchInfo,
    };
    use residuum::inference::ImageData;

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
        // HTTP response type for `GET /api/sessions` (its `SessionSummary`
        // items are also carried by the `session_started` frame).
        SessionListResponse::export_all(&cfg).unwrap();
        // HTTP response items for `GET /api/workbench/artifacts`.
        ArtifactSummary::export_all(&cfg).unwrap();
        // `GET /api/workbench/info` (its relay origins are exported with it).
        WorkbenchInfo::export_all(&cfg).unwrap();
        // The checkpoints API: `GET /api/checkpoints` (its `CheckpointSummary`
        // items and `CheckpointTrigger` are exported with it), `GET
        // /api/checkpoints/{id}` (its `ChangedPath`/`ChangeKind` are exported
        // with it), `GET /api/checkpoints/stats`, and the `POST
        // .../restore`/`.../undo` responses. `RepoKind` is the `repo` query
        // parameter/request-body field every checkpoints route takes.
        CheckpointPage::export_all(&cfg).unwrap();
        CheckpointDetail::export_all(&cfg).unwrap();
        RepoStats::export_all(&cfg).unwrap();
        RestoreOutcome::export_all(&cfg).unwrap();
        UndoOutcome::export_all(&cfg).unwrap();
        RepoKind::export_all(&cfg).unwrap();
        // `GET /api/scheduled/pulses` and `GET /api/scheduled/actions`.
        PulseInfo::export_all(&cfg).unwrap();
        ActionInfo::export_all(&cfg).unwrap();

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
