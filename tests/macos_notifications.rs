//! Integration tests for native macOS notification channel.
//!
//! Cross-platform config tests run on all platforms.
//! macOS-specific API tests are gated behind `cfg(target_os = "macos")`.

#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::indexing_slicing,
    reason = "test code uses indexing for clarity"
)]
#[expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live in tests/ directory, not inside #[cfg(test)] modules"
)]
#[expect(clippy::panic, reason = "test code uses panic for assertions")]
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "test code uses wildcard for unreachable branches"
)]
mod cross_platform {
    use residuum::notify::types::ExternalChannelKind;

    #[test]
    fn external_channel_kind_macos_variant_exists() {
        let kind = ExternalChannelKind::Macos {
            default_category: Some("alerts".to_string()),
            default_priority: Some("active".to_string()),
            throttle_window_secs: Some(30),
            sound: Some(true),
            app_name: Some("Test".to_string()),
            web_url: None,
        };

        match &kind {
            ExternalChannelKind::Macos {
                default_category, ..
            } => {
                assert_eq!(default_category.as_deref(), Some("alerts"));
            }
            _ => unreachable!("should be Macos variant"),
        }
    }

    #[test]
    fn macos_channel_config_loads_from_toml() {
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
                default_category,
                default_priority,
                throttle_window_secs,
                sound,
                app_name,
                web_url,
            } => {
                assert_eq!(default_category.as_deref(), Some("alerts"));
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

#[cfg(target_os = "macos")]
#[expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live in tests/ directory"
)]
mod macos_unit_tests {
    use residuum::bus::EventTrigger;
    use residuum::notify::macos::MacosChannelConfig;
    use residuum::notify::macos::categories::{
        MacosCategory, MacosInterruptionLevel, MacosNotificationAction,
        default_category_for_source, parse_category, parse_priority, resolve_category,
    };

    #[test]
    fn config_valid_minimal() {
        let cfg = MacosChannelConfig::default();
        assert!(cfg.validate().is_ok(), "default config should be valid");
    }

    #[test]
    fn config_valid_fully_specified() {
        let cfg = MacosChannelConfig {
            default_category: MacosCategory::Alerts,
            default_priority: MacosInterruptionLevel::TimeSensitive,
            throttle_window_secs: 10,
            sound: false,
            app_name: "TestApp".to_string(),
            web_url: Some("http://localhost:3000".to_string()),
        };
        assert!(
            cfg.validate().is_ok(),
            "fully specified config should be valid"
        );
    }

    #[test]
    fn config_invalid_throttle_zero() {
        let cfg = MacosChannelConfig {
            throttle_window_secs: 0,
            ..MacosChannelConfig::default()
        };
        assert!(cfg.validate().is_err(), "throttle 0 should fail validation");
    }

    #[test]
    fn config_invalid_throttle_over_max() {
        let cfg = MacosChannelConfig {
            throttle_window_secs: 301,
            ..MacosChannelConfig::default()
        };
        assert!(
            cfg.validate().is_err(),
            "throttle 301 should fail validation"
        );
    }

    #[test]
    fn parse_all_categories() {
        for cat in MacosCategory::all() {
            let id = cat.as_category_id();
            let parsed = parse_category(id).unwrap_or(MacosCategory::BackgroundResults);
            assert_eq!(*cat, parsed, "roundtrip failed for {id}");
        }
    }

    #[test]
    fn parse_all_priorities() {
        let levels = [
            ("passive", MacosInterruptionLevel::Passive),
            ("active", MacosInterruptionLevel::Active),
            ("time_sensitive", MacosInterruptionLevel::TimeSensitive),
        ];
        for (s, expected) in &levels {
            let parsed = parse_priority(s).unwrap_or(MacosInterruptionLevel::Active);
            assert_eq!(parsed, *expected, "parse failed for {s}");
        }
    }

    #[test]
    fn source_to_category_defaults() {
        assert_eq!(
            default_category_for_source(&EventTrigger::Pulse),
            MacosCategory::BackgroundResults
        );
        assert_eq!(
            default_category_for_source(&EventTrigger::Action),
            MacosCategory::Reminders
        );
        assert_eq!(
            default_category_for_source(&EventTrigger::Agent),
            MacosCategory::BackgroundResults
        );
    }

    #[test]
    fn resolve_uses_channel_default() {
        let resolved = resolve_category(&EventTrigger::Pulse, MacosCategory::Alerts);
        assert_eq!(
            resolved,
            MacosCategory::Alerts,
            "should use channel default"
        );
    }

    #[test]
    fn all_categories_have_actions() {
        for cat in MacosCategory::all() {
            let actions = MacosNotificationAction::for_category(*cat);
            assert!(
                !actions.is_empty(),
                "category {cat} should have at least one action"
            );
        }
    }

    #[test]
    fn inbox_items_has_mark_read() {
        let actions = MacosNotificationAction::for_category(MacosCategory::InboxItems);
        assert!(
            actions.contains(&MacosNotificationAction::MarkRead),
            "InboxItems should include MarkRead action"
        );
    }
}
