//! Smoke test: verify the binary starts without panicking.
//!
//! The binary will exit with an error (no config), which is expected.
//! We only check stderr for panic indicators.

#[expect(clippy::expect_used, reason = "test code uses expect for clarity")]
#[expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live in tests/ directory, not inside #[cfg(test)] modules"
)]
mod smoke {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    #[test]
    fn binary_starts_without_panic() {
        let mut child = Command::new(env!("CARGO_BIN_EXE_residuum"))
            .args(["serve", "--foreground"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn binary");

        // Let the process run long enough to initialize and potentially panic
        std::thread::sleep(Duration::from_secs(2));

        child.kill().expect("failed to kill child process");
        child.wait().expect("failed to wait on child process");

        let mut stderr_buf = String::new();
        child
            .stderr
            .take()
            .expect("missing stderr handle")
            .read_to_string(&mut stderr_buf)
            .expect("failed to read stderr");

        assert!(
            !stderr_buf.contains("panicked at"),
            "binary panicked on startup: {stderr_buf}"
        );
        assert!(
            !stderr_buf.contains("PANIC:"),
            "binary panicked on startup: {stderr_buf}"
        );
    }
}
