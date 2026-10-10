//! Test code that still waits on elapsed time, counted per file.
//!
//! `source_scan`'s `test_code_waits_on_events_not_elapsed_time` fails when a
//! file's count differs from what is listed here, so the counts can only go
//! down: converting a wait means lowering its file's entry, and a file at zero
//! comes off the list. The pre-commit hook refuses a commit that adds an entry.

/// Files whose test code waits on the clock, and how many such waits each has.
pub(super) const BACKLOG: &[(&str, usize)] = &[
    ("src/a2a/client/e2e_tests.rs", 2),
    ("src/a2a/client/siblings.rs", 3),
    ("src/a2a/server_e2e_tests.rs", 1),
    ("src/gateway/web/workbench.rs", 1),
    ("src/gateway/web/workspace_team_tests.rs", 1),
    ("src/hub/agent_watch/watcher.rs", 4),
    ("src/hub/host/tests.rs", 3),
    ("src/hub/host/tests/agent_watch.rs", 3),
    ("src/hub/host/tests/main_conversation.rs", 1),
    ("src/hub/host/tests/overview.rs", 1),
    ("src/hub/host/tests/push_triggers.rs", 1),
    ("src/hub/host/tests/restore.rs", 1),
    ("src/hub/host/tests/review_fixes.rs", 1),
    ("src/hub/http/tests/overview.rs", 3),
    ("src/hub/http/tests/session_relay.rs", 3),
    ("src/hub/relay_agents.rs", 2),
    ("src/hub/runtime.rs", 6),
    ("src/hub/team_events/recorder/tests.rs", 2),
    ("src/hub/test_support.rs", 1),
    ("src/interfaces/teams/setup_job.rs", 1),
    ("src/remote_access/caa.rs", 1),
    ("src/remote_access/siblings/service.rs", 1),
    ("src/remote_access/system_tests.rs", 1),
    ("src/tunnel/v2/tests.rs", 1),
    ("src/util/fs.rs", 1),
];

/// Test code where the clock is the stimulus rather than a guess, with why.
pub(super) const ALLOWED: &[(&str, usize, &str)] = &[
    (
        "src/inference/test_support.rs",
        1,
        "`Step::Pause` is a scripted network stall: the silence is the stimulus a stream's idle timeout is tested against",
    ),
    (
        "tests/smoke.rs",
        1,
        "runs the real binary for a while to see that it starts without panicking; it has no event of its own to wait on",
    ),
];
