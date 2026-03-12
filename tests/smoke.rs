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
    #[test]
    fn binary_starts_without_panic() {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_residuum"))
            .args(["serve", "--foreground"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .expect("failed to run binary");

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("panicked at"),
            "binary panicked on startup: {stderr}"
        );
        assert!(
            !stderr.contains("PANIC:"),
            "binary panicked on startup: {stderr}"
        );
    }
}
