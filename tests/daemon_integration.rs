//! Integration tests for daemon utilities (PID file management and process detection).

#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live in tests/ directory, not inside #[cfg(test)] modules"
)]
mod daemon_integration {
    use tempfile::tempdir;

    use residuum::daemon::{
        acquire_pid_lock, is_pid_locked, is_process_running, read_pid_file, remove_pid_file,
        write_pid_file,
    };

    #[test]
    fn pid_file_write_read_round_trip() {
        let dir = tempdir().unwrap();
        let pid_path = dir.path().join("test.pid");

        write_pid_file(&pid_path, 12345).unwrap();
        let read_back = read_pid_file(&pid_path).unwrap();
        assert_eq!(read_back, 12345, "pid should round-trip through write/read");
    }

    #[test]
    fn stale_pid_detection() {
        // PID 999_999_999 is almost certainly not running
        assert!(
            !is_process_running(999_999_999),
            "non-existent process should not be detected as running"
        );
    }

    #[test]
    fn self_detection() {
        let pid = std::process::id();
        assert!(
            is_process_running(pid),
            "current process should be detected as running"
        );
    }

    #[test]
    fn remove_pid_file_nonexistent_is_idempotent() {
        let dir = tempdir().unwrap();
        let pid_path = dir.path().join("nonexistent.pid");

        // Should succeed without error even though file doesn't exist
        remove_pid_file(&pid_path).unwrap();
    }

    #[test]
    fn remove_pid_file_actually_removes() {
        let dir = tempdir().unwrap();
        let pid_path = dir.path().join("test.pid");

        write_pid_file(&pid_path, 42).unwrap();
        assert!(pid_path.exists(), "pid file should exist after write");

        remove_pid_file(&pid_path).unwrap();
        assert!(!pid_path.exists(), "pid file should be gone after remove");
    }

    #[test]
    fn read_pid_file_missing_returns_error() {
        let dir = tempdir().unwrap();
        let pid_path = dir.path().join("missing.pid");

        assert!(
            read_pid_file(&pid_path).is_err(),
            "reading a missing pid file should return an error"
        );
    }

    #[test]
    fn write_pid_file_creates_parent_directories() {
        let dir = tempdir().unwrap();
        let pid_path = dir.path().join("nested").join("dirs").join("test.pid");

        write_pid_file(&pid_path, 99).unwrap();
        let read_back = read_pid_file(&pid_path).unwrap();
        assert_eq!(read_back, 99, "pid should be readable through nested dirs");
    }

    // --- File lock integration tests ---

    #[test]
    fn pid_lock_acquire_writes_current_pid() {
        let dir = tempdir().unwrap();
        let pid_path = dir.path().join("test.pid");
        let _lock = acquire_pid_lock(&pid_path).unwrap();

        let content = std::fs::read_to_string(&pid_path).unwrap();
        assert_eq!(
            content.trim().parse::<u32>().unwrap(),
            std::process::id(),
            "lock file should contain current PID"
        );
    }

    #[test]
    fn pid_lock_is_detected_by_is_pid_locked() {
        let dir = tempdir().unwrap();
        let pid_path = dir.path().join("test.pid");
        let _lock = acquire_pid_lock(&pid_path).unwrap();

        assert!(
            is_pid_locked(&pid_path).unwrap(),
            "held lock should be detected"
        );
    }

    #[test]
    fn pid_lock_stale_file_not_locked() {
        let dir = tempdir().unwrap();
        let pid_path = dir.path().join("test.pid");

        // Write a PID file without acquiring a lock
        write_pid_file(&pid_path, 99999).unwrap();

        assert!(
            !is_pid_locked(&pid_path).unwrap(),
            "unlocked pid file should be detected as stale"
        );
    }

    #[test]
    fn pid_lock_missing_file_not_locked() {
        let dir = tempdir().unwrap();
        let pid_path = dir.path().join("nonexistent.pid");

        assert!(
            !is_pid_locked(&pid_path).unwrap(),
            "missing file should not be detected as locked"
        );
    }

    #[test]
    fn pid_lock_double_acquire_fails() {
        let dir = tempdir().unwrap();
        let pid_path = dir.path().join("test.pid");
        let _lock = acquire_pid_lock(&pid_path).unwrap();

        let result = acquire_pid_lock(&pid_path);
        assert!(result.is_err(), "second lock acquisition should fail");
    }

    #[test]
    fn pid_lock_creates_parent_directories() {
        let dir = tempdir().unwrap();
        let pid_path = dir.path().join("nested").join("deep").join("test.pid");
        let _lock = acquire_pid_lock(&pid_path).unwrap();

        assert!(pid_path.exists(), "lock file should exist in nested dirs");
    }
}
