//! Tests for `/api/agents/{name}/usage/records` and `/usage/summary`, against
//! the fake directory's agents in every state. The records are written to the
//! agent's usage files the way the recorder writes them, so the routes read
//! what a real agent would have left.

use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use residuum_inference::{AttemptOutcome, Usage, UsageComponent, UsageRecord};

use super::*;

fn instant(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .unwrap()
        .with_timezone(&Utc)
}

fn call(
    component: UsageComponent,
    provider: &str,
    model: &str,
    at: DateTime<Utc>,
    outcome: AttemptOutcome,
    usage: Option<Usage>,
) -> UsageRecord {
    UsageRecord {
        recorded_at: at,
        component,
        conversation: None,
        provider: provider.to_string(),
        model: model.to_string(),
        outcome,
        usage,
    }
}

fn counts(uncached: u32, cache_read: u32, cache_write: u32, output: u32) -> Usage {
    Usage {
        uncached_input_tokens: Some(uncached),
        cache_read_tokens: Some(cache_read),
        cache_write_tokens: Some(cache_write),
        output_tokens: Some(output),
        ..Default::default()
    }
}

/// Four calls on 1 and 2 October 2026, one of them failed and one without usage.
fn october_calls() -> Vec<UsageRecord> {
    vec![
        call(
            UsageComponent::Main,
            "anthropic",
            "claude-opus",
            instant("2026-10-01T09:00:00Z"),
            AttemptOutcome::Succeeded,
            Some(counts(400, 400, 200, 10)),
        ),
        call(
            UsageComponent::Session {
                kind: "pulse".to_string(),
            },
            "anthropic",
            "claude-sonnet",
            instant("2026-10-01T10:00:00Z"),
            AttemptOutcome::Succeeded,
            Some(counts(100, 0, 0, 5)),
        ),
        call(
            UsageComponent::Main,
            "anthropic",
            "claude-opus",
            instant("2026-10-02T09:00:00Z"),
            AttemptOutcome::Failed,
            Some(counts(200, 0, 0, 0)),
        ),
        call(
            UsageComponent::Observer,
            "openai",
            "gpt",
            instant("2026-10-02T12:00:00Z"),
            AttemptOutcome::Succeeded,
            None,
        ),
    ]
}

/// Write `records` to the agent's usage files, one per UTC month, replacing
/// whatever they held.
fn seed_usage(agent_dir: &Path, records: &[UsageRecord]) {
    let dir = WorkspaceLayout::new(agent_dir).usage_records_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let mut by_month: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    for record in records {
        let file = format!("{}.jsonl", record.recorded_at.format("%Y-%m"));
        let text = by_month.entry(file).or_default();
        text.push_str(&serde_json::to_string(record).unwrap());
        text.push('\n');
    }
    for (file, text) in by_month {
        std::fs::write(dir.join(file), text).unwrap();
    }
}

/// The group of a summary's `groups` with `key`.
fn group<'a>(summary: &'a Value, key: &str) -> &'a Value {
    summary["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|group| group["key"] == key)
        .unwrap_or_else(|| panic!("no group {key} in {summary}"))
}

fn keys(summary: &Value) -> Vec<&str> {
    summary["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|group| group["key"].as_str().unwrap())
        .collect()
}

const OCTOBER: &str = "from=2026-10-01T00:00:00Z&to=2026-10-03T00:00:00Z";

#[tokio::test]
async fn usage_records_and_summaries_are_served_from_disk_in_every_agent_state() {
    let h = Harness::new();
    h.add_failed_agent();

    for name in EVERY_STATE {
        seed_usage(&h.agent_dir(name), &october_calls());

        let records = h
            .get_expect(
                &format!("/api/agents/{name}/usage/records?{OCTOBER}"),
                StatusCode::OK,
            )
            .await;
        assert_eq!(records["records"].as_array().unwrap().len(), 4, "{name}");

        let summary = h
            .get_expect(
                &format!("/api/agents/{name}/usage/summary?{OCTOBER}"),
                StatusCode::OK,
            )
            .await;
        assert_eq!(summary["total"]["attempts"], 4, "{name}");
    }
}

#[tokio::test]
async fn records_are_newest_first_with_their_fields_and_a_null_cost_when_unpriced() {
    let h = Harness::new();
    seed_usage(&h.agent_dir("scout"), &october_calls());

    let body = h
        .get_expect(
            &format!("/api/agents/scout/usage/records?{OCTOBER}"),
            StatusCode::OK,
        )
        .await;
    let records = body["records"].as_array().unwrap();
    let times: Vec<&str> = records
        .iter()
        .map(|record| record["recorded_at"].as_str().unwrap())
        .collect();
    assert_eq!(
        times,
        [
            "2026-10-02T12:00:00Z",
            "2026-10-02T09:00:00Z",
            "2026-10-01T10:00:00Z",
            "2026-10-01T09:00:00Z",
        ]
    );
    let newest = &records[0];
    assert_eq!(newest["component"], "observer");
    assert_eq!(newest["provider"], "openai");
    assert_eq!(newest["outcome"], "succeeded");
    assert!(
        newest.get("usage").is_none(),
        "a call with no usage has none"
    );
    assert_eq!(newest["estimated_cost_usd"], Value::Null);
    let failed = &records[1];
    assert_eq!(failed["outcome"], "failed");
    assert_eq!(failed["usage"]["uncached_input_tokens"], 200);
    assert_eq!(failed["estimated_cost_usd"], Value::Null);
    let session = &records[2];
    assert_eq!(session["component"]["session"]["kind"], "pulse");
}

#[tokio::test]
async fn records_default_to_the_last_week_ending_now() {
    let h = Harness::new();
    let now = Utc::now();
    seed_usage(
        &h.agent_dir("scout"),
        &[
            call(
                UsageComponent::Main,
                "anthropic",
                "claude-opus",
                now - Duration::hours(1),
                AttemptOutcome::Succeeded,
                None,
            ),
            call(
                UsageComponent::Session {
                    kind: "pulse".to_string(),
                },
                "anthropic",
                "claude-sonnet",
                now - Duration::hours(2),
                AttemptOutcome::Succeeded,
                None,
            ),
            call(
                UsageComponent::Main,
                "anthropic",
                "claude-opus",
                now - Duration::days(3),
                AttemptOutcome::Succeeded,
                None,
            ),
            call(
                UsageComponent::Observer,
                "openai",
                "gpt",
                now - Duration::days(30),
                AttemptOutcome::Succeeded,
                None,
            ),
        ],
    );

    let body = h
        .get_expect("/api/agents/scout/usage/records", StatusCode::OK)
        .await;
    let records = body["records"].as_array().unwrap();
    let components: Vec<&Value> = records.iter().map(|record| &record["component"]).collect();
    assert_eq!(
        components,
        [
            &json!("main"),
            &json!({ "session": { "kind": "pulse" } }),
            &json!("main")
        ],
        "the 30-day-old observer call is outside the default week"
    );
}

#[tokio::test]
async fn records_filter_by_component_and_read_a_half_open_range() {
    let h = Harness::new();
    seed_usage(&h.agent_dir("scout"), &october_calls());

    // `to` is excluded: the failed main call at 09:00 on the 2nd is out.
    let body = h
        .get_expect(
            "/api/agents/scout/usage/records?from=2026-10-01T09:00:00Z&to=2026-10-02T09:00:00Z&component=main",
            StatusCode::OK,
        )
        .await;
    let records = body["records"].as_array().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0]["recorded_at"], "2026-10-01T09:00:00Z",
        "`from` is included"
    );

    let main_only = h
        .get_expect(
            &format!("/api/agents/scout/usage/records?{OCTOBER}&component=main"),
            StatusCode::OK,
        )
        .await;
    assert_eq!(main_only["records"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn records_are_limited_to_5000_and_default_to_500() {
    let h = Harness::new();
    let base = instant("2026-10-01T09:00:00Z");
    let calls: Vec<UsageRecord> = (0..5001)
        .map(|second| {
            call(
                UsageComponent::Main,
                "anthropic",
                "claude-opus",
                base + Duration::seconds(second),
                AttemptOutcome::Succeeded,
                None,
            )
        })
        .collect();
    seed_usage(&h.agent_dir("scout"), &calls);

    let default = h
        .get_expect(
            &format!("/api/agents/scout/usage/records?{OCTOBER}"),
            StatusCode::OK,
        )
        .await;
    let records = default["records"].as_array().unwrap();
    assert_eq!(records.len(), 500, "the default limit");
    assert_eq!(
        records[0]["recorded_at"], "2026-10-01T10:23:20Z",
        "the newest call comes first (09:00:00 plus 5000 seconds)"
    );

    let clamped = h
        .get_expect(
            &format!("/api/agents/scout/usage/records?{OCTOBER}&limit=99999"),
            StatusCode::OK,
        )
        .await;
    assert_eq!(clamped["records"].as_array().unwrap().len(), 5000);
}

#[tokio::test]
async fn bad_query_parameters_are_a_400_with_a_message() {
    let h = Harness::new();
    for uri in [
        "/api/agents/scout/usage/records?from=yesterday",
        "/api/agents/scout/usage/records?from=2026-10-02T00:00:00Z&to=2026-10-01T00:00:00Z",
        "/api/agents/scout/usage/records?component=observers",
        "/api/agents/scout/usage/records?limit=0",
        "/api/agents/scout/usage/records?limit=lots",
        "/api/agents/scout/usage/summary?group_by=hour",
        "/api/agents/scout/usage/summary?to=2026-10-01",
    ] {
        let body = h.get_expect(uri, StatusCode::BAD_REQUEST).await;
        assert!(
            body["error"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "{uri}: {body}"
        );
    }
}

#[tokio::test]
async fn a_summary_groups_by_component_with_hit_rates_and_a_total() {
    let h = Harness::new();
    seed_usage(&h.agent_dir("scout"), &october_calls());

    let summary = h
        .get_expect(
            &format!("/api/agents/scout/usage/summary?{OCTOBER}"),
            StatusCode::OK,
        )
        .await;
    assert_eq!(summary["from"], "2026-10-01T00:00:00Z");
    assert_eq!(summary["to"], "2026-10-03T00:00:00Z");
    assert_eq!(keys(&summary), ["main", "observer", "session"]);

    let main = group(&summary, "main");
    assert_eq!(main["attempts"], 2);
    assert_eq!(main["failed_attempts"], 1);
    assert_eq!(main["uncached_input_tokens"], 600);
    assert_eq!(main["cache_read_tokens"], 400);
    assert_eq!(main["cache_write_tokens"], 200);
    assert_eq!(main["output_tokens"], 10);
    assert_eq!(main["reasoning_tokens"], 0);
    assert_eq!(main["cache_hit_rate"], json!(0.333_333));
    assert_eq!(main["unpriced_attempts"], 2);

    let observer = group(&summary, "observer");
    assert_eq!(observer["attempts"], 1);
    assert_eq!(
        observer["cache_hit_rate"],
        Value::Null,
        "no input was reported"
    );

    assert_eq!(summary["total"]["key"], "total");
    assert_eq!(summary["total"]["attempts"], 4);
    assert_eq!(summary["total"]["failed_attempts"], 1);
    assert_eq!(summary["total"]["uncached_input_tokens"], 700);
    assert_eq!(summary["total"]["estimated_cost_usd"], Value::Null);
    assert_eq!(summary["total"]["unpriced_attempts"], 3);
}

#[tokio::test]
async fn a_summary_groups_by_model_and_by_utc_day() {
    let h = Harness::new();
    seed_usage(&h.agent_dir("scout"), &october_calls());

    let by_model = h
        .get_expect(
            &format!("/api/agents/scout/usage/summary?{OCTOBER}&group_by=model"),
            StatusCode::OK,
        )
        .await;
    assert_eq!(
        keys(&by_model),
        [
            "anthropic/claude-opus",
            "anthropic/claude-sonnet",
            "openai/gpt"
        ]
    );
    assert_eq!(group(&by_model, "anthropic/claude-opus")["attempts"], 2);

    let by_day = h
        .get_expect(
            &format!("/api/agents/scout/usage/summary?{OCTOBER}&group_by=day"),
            StatusCode::OK,
        )
        .await;
    assert_eq!(keys(&by_day), ["2026-10-01", "2026-10-02"]);
    assert_eq!(group(&by_day, "2026-10-01")["attempts"], 2);
    assert_eq!(group(&by_day, "2026-10-02")["attempts"], 2);
}

#[tokio::test]
async fn an_empty_summary_has_no_groups_and_null_rates() {
    let h = Harness::new();
    let summary = h
        .get_expect(
            "/api/agents/quiet/usage/summary?from=2026-10-01T00:00:00Z&to=2026-10-02T00:00:00Z",
            StatusCode::OK,
        )
        .await;
    assert_eq!(summary["groups"], json!([]));
    assert_eq!(summary["total"]["attempts"], 0);
    assert_eq!(summary["total"]["cache_hit_rate"], Value::Null);
    assert_eq!(summary["total"]["estimated_cost_usd"], Value::Null);
}

#[tokio::test]
async fn an_unreadable_usage_directory_is_a_500_not_an_empty_answer() {
    let h = Harness::new();
    let agent = h.agent_dir("scout");
    let dir = WorkspaceLayout::new(&agent).usage_records_dir();
    std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
    // A file where the directory should be: the records can't be listed.
    std::fs::write(&dir, "not a directory").unwrap();

    h.get_expect(
        &format!("/api/agents/scout/usage/records?{OCTOBER}"),
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .await;
    h.get_expect(
        &format!("/api/agents/scout/usage/summary?{OCTOBER}"),
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .await;
}
