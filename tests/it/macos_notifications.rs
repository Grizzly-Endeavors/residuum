//! Integration tests for the native macOS notification channel's configuration.
//!
//! These run on every platform because they exercise TOML parsing, not the
//! macOS APIs. Behavior of the macOS types themselves is covered by the unit
//! tests beside the code in `src/notify/macos/`.

#[expect(
    clippy::indexing_slicing,
    reason = "test code uses indexing for clarity"
)]
#[expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live in tests/ directory, not inside #[cfg(test)] modules"
)]
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "test code uses wildcard for unreachable branches"
)]
mod cross_platform {
    use residuum::notify::types::ExternalChannelKind;

    #[test]
    fn external_channel_kind_macos_variant_exists() {
        let kind = ExternalChannelKind::Macos {
            default_priority: Some("active".to_string()),
            throttle_window_secs: Some(30),
            sound: Some(true),
            app_name: Some("Test".to_string()),
            web_url: None,
        };

        match &kind {
            ExternalChannelKind::Macos {
                default_priority, ..
            } => {
                assert_eq!(default_priority.as_deref(), Some("active"));
            }
            _ => unreachable!("should be Macos variant"),
        }
    }

    #[test]
    fn macos_channel_config_loads_from_toml() {
        // `default_category` is retired. It stays in this fixture on purpose:
        // a config still carrying it must load, warn, and ignore the key rather
        // than fail startup.
        let toml_str = r#"
[channels.macos]
type = "macos"
default_category = "alerts"
default_priority = "time_sensitive"
throttle_window_secs = 10
sound = true
app_name = "Residuum"
web_url = "http://localhost:3000"
"#;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("channels.toml");
        std::fs::write(&path, toml_str).unwrap();

        let configs = residuum::workspace::config::load_channel_configs(&path).unwrap();

        assert_eq!(configs.len(), 1, "should parse one channel config");
        let cfg = &configs[0];

        match &cfg.kind {
            ExternalChannelKind::Macos {
                default_priority,
                throttle_window_secs,
                sound,
                app_name,
                web_url,
            } => {
                assert_eq!(default_priority.as_deref(), Some("time_sensitive"));
                assert_eq!(*throttle_window_secs, Some(10));
                assert_eq!(*sound, Some(true));
                assert_eq!(app_name.as_deref(), Some("Residuum"));
                assert_eq!(web_url.as_deref(), Some("http://localhost:3000"));
            }
            _ => panic!("expected Macos channel kind"),
        }
    }
}
