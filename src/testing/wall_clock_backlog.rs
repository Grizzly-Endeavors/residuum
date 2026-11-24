pub(super) const BACKLOG: &[(&str, usize)] = &[];

pub(super) const ALLOWED: &[(&str, usize, &str)] = &[
    (
        "tests/it/a2a_server.rs",
        1,
        "the harness's `Drop` removes the workspace while detached session and checkpoint writers may still be finishing; teardown is synchronous, with nothing to await their end on, so the removal retries between short pauses",
    ),
    (
        "crates/inference/src/test_support.rs",
        1,
        "`Step::Pause` is a scripted network stall: the silence is the stimulus a stream's idle timeout is tested against",
    ),
    (
        "tests/it/smoke.rs",
        1,
        "runs the real binary for a while to see that it starts without panicking; it has no event of its own to wait on",
    ),
];
