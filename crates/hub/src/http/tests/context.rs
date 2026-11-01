//! Tests for `/api/agents/{name}/context`, against the fake directory's
//! agents in every state. The snapshot is written to the agent's workspace
//! the way the agent saves it after a turn, so the route reads what a real
//! agent would have left.

use residuum_agent::context::{ContextBreakdown, ContextSnapshot};

use super::*;

fn snapshot() -> ContextSnapshot {
    ContextSnapshot {
        computed_at: "2026-11-01T12:00:00Z".parse().unwrap(),
        breakdown: ContextBreakdown {
            harness_tokens: 900,
            history_tokens: 120,
            history_count: 3,
            status_line_tokens: 25,
            total_tokens: 1045,
            ..ContextBreakdown::default()
        },
    }
}

/// Write `contents` where `name`'s agent saves its context breakdown.
async fn write_breakdown(h: &Harness, name: &str, contents: &str) {
    let path = WorkspaceLayout::new(h.agent_dir(name)).context_breakdown_json();
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&path, contents).await.unwrap();
}

#[tokio::test]
async fn the_latest_breakdown_is_served_from_disk_in_every_agent_state() {
    let h = Harness::new();
    h.add_failed_agent();
    let saved = snapshot();

    for name in EVERY_STATE {
        let uri = format!("/api/agents/{name}/context");
        let missing = h.get_expect(&uri, StatusCode::NOT_FOUND).await;
        assert!(
            missing["error"]
                .as_str()
                .unwrap()
                .contains("no context breakdown yet"),
            "{name}: {missing}"
        );

        write_breakdown(&h, name, &serde_json::to_string(&saved).unwrap()).await;
        let served = h.get_expect(&uri, StatusCode::OK).await;
        assert_eq!(served["computed_at"], "2026-11-01T12:00:00Z", "{name}");
        assert_eq!(served["harness_tokens"], 900, "{name}");
        assert_eq!(served["status_line_tokens"], 25, "{name}");
        assert_eq!(served["total_tokens"], 1045, "{name}");
        assert_eq!(
            served,
            serde_json::to_value(&saved).unwrap(),
            "{name}: the snapshot as saved, its fields beside the time"
        );
    }
}

#[tokio::test]
async fn an_unreadable_breakdown_is_a_500_not_a_missing_one() {
    let h = Harness::new();
    write_breakdown(&h, "quiet", "not json").await;

    let answer = h
        .get_expect(
            "/api/agents/quiet/context",
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .await;
    assert!(
        answer["error"]
            .as_str()
            .unwrap()
            .contains("couldn't read the context breakdown"),
        "{answer}"
    );
}
