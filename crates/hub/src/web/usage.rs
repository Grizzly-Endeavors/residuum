//! The usage record routes: one agent's model calls, read from its usage
//! records and priced at the hub's current prices.
//!
//! The hub builds this router with each request's file router (see
//! `AgentHost::file_router_for`), so the prices are the ones in the hub config
//! as it is now, and a running agent's router never needs them.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use residuum_inference::{AttemptOutcome, PriceTable, UsageComponent, UsageRecord, UsageRecorder};

/// The window read when a request names no `from`: the last seven days.
const DEFAULT_WINDOW_DAYS: i64 = 7;
/// How many records `/usage/records` returns when it isn't given a `limit`.
const DEFAULT_LIMIT: usize = 500;
/// The most records one `/usage/records` answer holds; a larger `limit` is
/// clamped to this.
const MAX_LIMIT: usize = 5000;

/// What the usage routes read: one agent's usage records and the prices they
/// are estimated at.
#[derive(Clone)]
pub(crate) struct UsageState {
    pub(crate) recorder: UsageRecorder,
    pub(crate) prices: Arc<PriceTable>,
}

/// The usage record routes, over one agent's state.
pub(crate) fn agent_usage_api_router(state: UsageState) -> axum::Router {
    axum::Router::new()
        .route("/api/usage/records", get(api_usage_records))
        .route("/api/usage/summary", get(api_usage_summary))
        .with_state(state)
}

/// Query parameters for `GET /api/agents/{name}/usage/records`.
#[derive(Debug, Deserialize)]
pub(super) struct RecordsQuery {
    from: Option<String>,
    to: Option<String>,
    component: Option<String>,
    limit: Option<String>,
}

/// Query parameters for `GET /api/agents/{name}/usage/summary`.
#[derive(Debug, Deserialize)]
pub(super) struct SummaryQuery {
    from: Option<String>,
    to: Option<String>,
    group_by: Option<String>,
}

/// One usage record as the routes answer it: the record's fields, and what
/// its usage is estimated to have cost, `null` when it has no usage or no
/// price for its model.
#[derive(Debug, Serialize)]
pub(super) struct RecordView {
    #[serde(flatten)]
    record: UsageRecord,
    estimated_cost_usd: Option<f64>,
}

/// `GET /api/agents/{name}/usage/records` answer: `{ "records": [...] }`,
/// newest first.
#[derive(Debug, Serialize)]
pub(super) struct RecordsResponse {
    records: Vec<RecordView>,
}

/// One group of a usage summary: token totals, cache hit rate and cost
/// across the calls that fall in it.
#[derive(Debug, Serialize, PartialEq)]
pub(super) struct UsageGroup {
    key: String,
    calls: u64,
    failed_calls: u64,
    uncached_input_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    output_tokens: u64,
    reasoning_tokens: u64,
    /// Cache reads as a share of all input tokens, `null` when the calls
    /// reported no input.
    cache_hit_rate: Option<f64>,
    /// The cost of the priced calls; `null` when none of them was priced.
    estimated_cost_usd: Option<f64>,
    /// Calls that reported usage but have no price for their model.
    unpriced_calls: u64,
}

/// What a summary groups its calls by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GroupBy {
    Component,
    Model,
    Day,
}

impl GroupBy {
    fn parse(text: Option<&str>) -> Result<Self, String> {
        match text {
            None | Some("component") => Ok(Self::Component),
            Some("model") => Ok(Self::Model),
            Some("day") => Ok(Self::Day),
            Some(other) => Err(format!(
                "group_by must be component, model or day, not '{other}'"
            )),
        }
    }

    fn key(self, record: &UsageRecord) -> String {
        match self {
            Self::Component => record.component.name().to_string(),
            Self::Model => format!("{}/{}", record.provider, record.model),
            Self::Day => record.recorded_at.format("%Y-%m-%d").to_string(),
        }
    }
}

/// `GET /api/agents/{name}/usage/summary` answer.
#[derive(Debug, Serialize)]
pub(super) struct SummaryResponse {
    from: String,
    to: String,
    groups: Vec<UsageGroup>,
    total: UsageGroup,
}

/// The summed counts of the calls in one group.
#[derive(Debug, Default)]
struct Tally {
    calls: u64,
    failed_calls: u64,
    uncached_input_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    output_tokens: u64,
    reasoning_tokens: u64,
    estimated_cost_usd: Option<f64>,
    unpriced_calls: u64,
}

impl Tally {
    /// Count one call, priced at `cost` when it has a price.
    fn add(&mut self, record: &UsageRecord, cost: Option<f64>) {
        self.calls += 1;
        if record.outcome == AttemptOutcome::Failed {
            self.failed_calls += 1;
        }
        let Some(usage) = &record.usage else {
            return;
        };
        self.uncached_input_tokens += u64::from(usage.uncached_input_tokens.unwrap_or(0));
        self.cache_read_tokens += u64::from(usage.cache_read_tokens.unwrap_or(0));
        self.cache_write_tokens += u64::from(usage.cache_write_tokens.unwrap_or(0));
        self.output_tokens += u64::from(usage.output_tokens.unwrap_or(0));
        self.reasoning_tokens += u64::from(usage.reasoning_tokens.unwrap_or(0));
        match cost {
            Some(usd) => {
                self.estimated_cost_usd = Some(self.estimated_cost_usd.unwrap_or(0.0) + usd)
            }
            None => self.unpriced_calls += 1,
        }
    }

    fn into_group(self, key: String) -> UsageGroup {
        let input = self.uncached_input_tokens + self.cache_read_tokens + self.cache_write_tokens;
        UsageGroup {
            key,
            calls: self.calls,
            failed_calls: self.failed_calls,
            uncached_input_tokens: self.uncached_input_tokens,
            cache_read_tokens: self.cache_read_tokens,
            cache_write_tokens: self.cache_write_tokens,
            output_tokens: self.output_tokens,
            reasoning_tokens: self.reasoning_tokens,
            cache_hit_rate: share(self.cache_read_tokens, input),
            estimated_cost_usd: self.estimated_cost_usd,
            unpriced_calls: self.unpriced_calls,
        }
    }
}

/// `part` as a fraction of `whole`, to six decimal places, or `None` when
/// `whole` is zero. `part` must not exceed `whole`.
///
/// The division is done in integers, so no count passes through a lossy
/// float cast: the quotient in millionths is at most 1,000,000, which an
/// `f64` holds exactly.
fn share(part: u64, whole: u64) -> Option<f64> {
    if whole == 0 {
        return None;
    }
    let millionths = u128::from(part) * 1_000_000 / u128::from(whole);
    u32::try_from(millionths)
        .ok()
        .map(|millionths| f64::from(millionths) / 1_000_000.0)
}

/// The groups of a summary, by key, and the total of every call.
#[derive(Debug, PartialEq)]
pub(super) struct Summary {
    groups: Vec<UsageGroup>,
    total: UsageGroup,
}

/// Group `records` by `group_by` and total each group. `cost` gives a call's
/// estimated cost, `None` when the call is unpriced.
///
/// Groups come out sorted by key. The total's key is `total`.
pub(super) fn summarize(
    records: &[UsageRecord],
    group_by: GroupBy,
    cost: impl Fn(&UsageRecord) -> Option<f64>,
) -> Summary {
    let mut groups: BTreeMap<String, Tally> = BTreeMap::new();
    let mut total = Tally::default();
    for record in records {
        let cost = cost(record);
        groups
            .entry(group_by.key(record))
            .or_default()
            .add(record, cost);
        total.add(record, cost);
    }
    Summary {
        groups: groups
            .into_iter()
            .map(|(key, tally)| tally.into_group(key))
            .collect(),
        total: total.into_group("total".to_string()),
    }
}

/// The estimated cost of one record, `None` when it has no usage or its
/// model has no price.
fn estimated_cost_usd(prices: &PriceTable, record: &UsageRecord) -> Option<f64> {
    let usage = record.usage.as_ref()?;
    prices
        .estimate(&record.provider, &record.model, usage)
        .map(|estimate| estimate.usd)
}

/// A time window, from `from` up to but not including `to`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Range {
    from: DateTime<Utc>,
    to: DateTime<Utc>,
}

/// The window a request names. `to` defaults to `now` and `from` to a week
/// before `to`.
fn resolve_range(
    from: Option<&str>,
    to: Option<&str>,
    now: DateTime<Utc>,
) -> Result<Range, String> {
    let to = match to {
        Some(text) => parse_instant("to", text)?,
        None => now,
    };
    let from = match from {
        Some(text) => parse_instant("from", text)?,
        None => to
            .checked_sub_signed(chrono::Duration::days(DEFAULT_WINDOW_DAYS))
            .ok_or_else(|| "to is too early to look back a week from".to_string())?,
    };
    if from >= to {
        return Err("from must be earlier than to".to_string());
    }
    Ok(Range { from, to })
}

fn parse_instant(name: &str, text: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(text)
        .map(|instant| instant.with_timezone(&Utc))
        .map_err(|_| format!("{name} must be an RFC 3339 time, such as 2026-10-01T00:00:00Z"))
}

/// The `limit` a request names: at least one, at most [`MAX_LIMIT`].
fn parse_limit(text: Option<&str>) -> Result<usize, String> {
    match text {
        None => Ok(DEFAULT_LIMIT),
        Some(text) => text
            .parse::<usize>()
            .ok()
            .filter(|limit| *limit >= 1)
            .map(|limit| limit.min(MAX_LIMIT))
            .ok_or_else(|| "limit must be a whole number of at least 1".to_string()),
    }
}

fn parse_component(text: Option<&str>) -> Result<Option<&str>, String> {
    match text {
        None => Ok(None),
        Some(name) if UsageComponent::NAMES.iter().any(|known| *known == name) => Ok(Some(name)),
        Some(name) => Err(format!(
            "component must be one of {}, not '{name}'",
            UsageComponent::NAMES.join(", ")
        )),
    }
}

fn error_response(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into() }))).into_response()
}

fn bad_request(message: String) -> Response {
    error_response(StatusCode::BAD_REQUEST, message)
}

/// The records in `range`, or a `500` naming the failure. A read error is
/// never an empty answer.
async fn load_records(
    recorder: &UsageRecorder,
    range: Range,
    component: Option<&str>,
) -> Result<Vec<UsageRecord>, Response> {
    recorder
        .load(range.from, range.to, component)
        .await
        .map_err(|e| {
            tracing::error!(error = ?e, "failed to read the usage records");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "couldn't read the usage records; the details are in Residuum's logs",
            )
        })
}

/// Sort newest first. Records written in the same instant keep the order
/// they were written in, latest first.
fn newest_first(records: &mut [UsageRecord]) {
    records.reverse();
    records.sort_by_key(|record| std::cmp::Reverse(record.recorded_at));
}

fn rfc3339(instant: DateTime<Utc>) -> String {
    instant.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// `GET /api/agents/{name}/usage/records` — the agent's usage records in the
/// window, newest first, up to `limit`.
pub(super) async fn api_usage_records(
    State(state): State<UsageState>,
    Query(query): Query<RecordsQuery>,
) -> Result<Json<RecordsResponse>, Response> {
    let range = resolve_range(query.from.as_deref(), query.to.as_deref(), Utc::now())
        .map_err(bad_request)?;
    let limit = parse_limit(query.limit.as_deref()).map_err(bad_request)?;
    let component = parse_component(query.component.as_deref()).map_err(bad_request)?;

    let mut records = load_records(&state.recorder, range, component).await?;
    newest_first(&mut records);
    records.truncate(limit);
    let records = records
        .into_iter()
        .map(|record| RecordView {
            estimated_cost_usd: estimated_cost_usd(&state.prices, &record),
            record,
        })
        .collect();
    Ok(Json(RecordsResponse { records }))
}

/// `GET /api/agents/{name}/usage/summary` — the agent's calls in the window,
/// totalled by component, model or UTC day.
pub(super) async fn api_usage_summary(
    State(state): State<UsageState>,
    Query(query): Query<SummaryQuery>,
) -> Result<Json<SummaryResponse>, Response> {
    let range = resolve_range(query.from.as_deref(), query.to.as_deref(), Utc::now())
        .map_err(bad_request)?;
    let group_by = GroupBy::parse(query.group_by.as_deref()).map_err(bad_request)?;

    let records = load_records(&state.recorder, range, None).await?;
    let summary = summarize(&records, group_by, |record| {
        estimated_cost_usd(&state.prices, record)
    });
    Ok(Json(SummaryResponse {
        from: rfc3339(range.from),
        to: rfc3339(range.to),
        groups: summary.groups,
        total: summary.total,
    }))
}

#[cfg(test)]
mod tests {
    use residuum_inference::Usage;

    use super::*;

    fn instant(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn record(
        component: UsageComponent,
        provider: &str,
        model: &str,
        at: &str,
        outcome: AttemptOutcome,
        usage: Option<Usage>,
    ) -> UsageRecord {
        UsageRecord {
            recorded_at: instant(at),
            component,
            conversation: None,
            provider: provider.to_string(),
            model: model.to_string(),
            outcome,
            usage,
        }
    }

    fn counts(uncached: u32, cache_read: u32, cache_write: u32, output: u32) -> Option<Usage> {
        Some(Usage {
            uncached_input_tokens: Some(uncached),
            cache_read_tokens: Some(cache_read),
            cache_write_tokens: Some(cache_write),
            output_tokens: Some(output),
            ..Default::default()
        })
    }

    fn no_price(_: &UsageRecord) -> Option<f64> {
        None
    }

    #[test]
    fn a_range_defaults_to_the_week_before_now() {
        let now = instant("2026-10-10T12:00:00Z");
        assert_eq!(
            resolve_range(None, None, now),
            Ok(Range {
                from: instant("2026-10-03T12:00:00Z"),
                to: now,
            })
        );
    }

    #[test]
    fn a_range_takes_the_bounds_it_names() {
        let now = instant("2026-10-10T12:00:00Z");
        assert_eq!(
            resolve_range(Some("2026-10-01T00:00:00Z"), None, now),
            Ok(Range {
                from: instant("2026-10-01T00:00:00Z"),
                to: now,
            })
        );
        assert_eq!(
            resolve_range(None, Some("2026-10-05T00:00:00+02:00"), now),
            Ok(Range {
                from: instant("2026-09-27T22:00:00Z"),
                to: instant("2026-10-04T22:00:00Z"),
            })
        );
    }

    #[test]
    fn a_range_rejects_bad_times_and_empty_windows() {
        let now = instant("2026-10-10T12:00:00Z");
        assert!(resolve_range(Some("yesterday"), None, now).is_err());
        assert!(resolve_range(None, Some("2026-10-10"), now).is_err());
        assert!(
            resolve_range(
                Some("2026-10-10T00:00:00Z"),
                Some("2026-10-10T00:00:00Z"),
                now
            )
            .is_err()
        );
        assert!(
            resolve_range(
                Some("2026-10-11T00:00:00Z"),
                Some("2026-10-10T00:00:00Z"),
                now
            )
            .is_err()
        );
    }

    #[test]
    fn a_limit_defaults_and_clamps_to_the_maximum() {
        assert_eq!(parse_limit(None), Ok(500));
        assert_eq!(parse_limit(Some("20")), Ok(20));
        assert_eq!(parse_limit(Some("5000")), Ok(5000));
        assert_eq!(parse_limit(Some("5001")), Ok(5000));
        assert_eq!(parse_limit(Some("999999")), Ok(5000));
    }

    #[test]
    fn a_limit_must_be_a_whole_number_of_at_least_one() {
        for bad in ["0", "-3", "1.5", "many", ""] {
            assert!(parse_limit(Some(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_component_filter_must_name_a_component() {
        assert_eq!(parse_component(None), Ok(None));
        assert_eq!(parse_component(Some("observer")), Ok(Some("observer")));
        assert!(parse_component(Some("observers")).is_err());
    }

    #[test]
    fn group_by_defaults_to_component_and_rejects_unknown_keys() {
        assert_eq!(GroupBy::parse(None), Ok(GroupBy::Component));
        assert_eq!(GroupBy::parse(Some("model")), Ok(GroupBy::Model));
        assert_eq!(GroupBy::parse(Some("day")), Ok(GroupBy::Day));
        assert!(GroupBy::parse(Some("hour")).is_err());
    }

    #[test]
    fn summaries_group_by_component_in_key_order_with_a_total() {
        let records = [
            record(
                UsageComponent::WebModel,
                "a",
                "m",
                "2026-10-02T09:00:00Z",
                AttemptOutcome::Succeeded,
                counts(10, 0, 0, 5),
            ),
            record(
                UsageComponent::Main,
                "a",
                "m",
                "2026-10-01T09:00:00Z",
                AttemptOutcome::Succeeded,
                counts(1, 2, 3, 4),
            ),
            record(
                UsageComponent::Main,
                "a",
                "m",
                "2026-10-01T10:00:00Z",
                AttemptOutcome::Succeeded,
                counts(5, 6, 7, 8),
            ),
        ];
        let summary = summarize(&records, GroupBy::Component, no_price);
        let keys: Vec<&str> = summary.groups.iter().map(|g| g.key.as_str()).collect();
        assert_eq!(keys, ["main", "web_model"]);
        let main = &summary.groups[0];
        assert_eq!(main.calls, 2);
        assert_eq!(main.uncached_input_tokens, 6);
        assert_eq!(main.cache_read_tokens, 8);
        assert_eq!(main.cache_write_tokens, 10);
        assert_eq!(main.output_tokens, 12);
        assert_eq!(summary.total.key, "total");
        assert_eq!(summary.total.calls, 3);
        assert_eq!(summary.total.uncached_input_tokens, 16);
        assert_eq!(summary.total.output_tokens, 17);
    }

    #[test]
    fn summaries_group_by_provider_and_model() {
        let records = [
            record(
                UsageComponent::Main,
                "anthropic",
                "opus",
                "2026-10-01T09:00:00Z",
                AttemptOutcome::Succeeded,
                counts(1, 0, 0, 1),
            ),
            record(
                UsageComponent::Observer,
                "anthropic",
                "opus",
                "2026-10-01T09:30:00Z",
                AttemptOutcome::Succeeded,
                counts(1, 0, 0, 1),
            ),
            record(
                UsageComponent::Main,
                "openai",
                "gpt",
                "2026-10-01T10:00:00Z",
                AttemptOutcome::Succeeded,
                counts(1, 0, 0, 1),
            ),
        ];
        let summary = summarize(&records, GroupBy::Model, no_price);
        let keys: Vec<(&str, u64)> = summary
            .groups
            .iter()
            .map(|g| (g.key.as_str(), g.calls))
            .collect();
        assert_eq!(keys, [("anthropic/opus", 2), ("openai/gpt", 1)]);
    }

    #[test]
    fn summaries_group_by_the_utc_day_of_each_call() {
        let records = [
            record(
                UsageComponent::Main,
                "a",
                "m",
                "2026-10-01T23:59:59Z",
                AttemptOutcome::Succeeded,
                None,
            ),
            record(
                UsageComponent::Main,
                "a",
                "m",
                "2026-10-02T00:00:00Z",
                AttemptOutcome::Succeeded,
                None,
            ),
            record(
                UsageComponent::Main,
                "a",
                "m",
                "2026-10-02T12:00:00+05:00",
                AttemptOutcome::Succeeded,
                None,
            ),
        ];
        let summary = summarize(&records, GroupBy::Day, no_price);
        let keys: Vec<(&str, u64)> = summary
            .groups
            .iter()
            .map(|g| (g.key.as_str(), g.calls))
            .collect();
        assert_eq!(keys, [("2026-10-01", 1), ("2026-10-02", 2)]);
    }

    #[test]
    fn cache_hit_rate_is_cache_reads_over_all_input_tokens() {
        let records = [record(
            UsageComponent::Main,
            "a",
            "m",
            "2026-10-01T09:00:00Z",
            AttemptOutcome::Succeeded,
            counts(500, 250, 250, 10),
        )];
        let summary = summarize(&records, GroupBy::Component, no_price);
        assert_eq!(summary.groups[0].cache_hit_rate, Some(0.25));
        assert_eq!(summary.total.cache_hit_rate, Some(0.25));
    }

    #[test]
    fn cache_hit_rate_is_null_when_no_input_was_reported() {
        let records = [
            record(
                UsageComponent::Main,
                "a",
                "m",
                "2026-10-01T09:00:00Z",
                AttemptOutcome::Failed,
                None,
            ),
            record(
                UsageComponent::Main,
                "a",
                "m",
                "2026-10-01T09:05:00Z",
                AttemptOutcome::Succeeded,
                counts(0, 0, 0, 40),
            ),
        ];
        let summary = summarize(&records, GroupBy::Component, no_price);
        assert_eq!(summary.groups[0].cache_hit_rate, None);
        assert_eq!(summary.total.cache_hit_rate, None);
    }

    #[test]
    fn cost_sums_priced_calls_and_counts_the_unpriced_ones_with_usage() {
        let records = [
            record(
                UsageComponent::Main,
                "priced",
                "m",
                "2026-10-01T09:00:00Z",
                AttemptOutcome::Succeeded,
                counts(1, 0, 0, 1),
            ),
            record(
                UsageComponent::Main,
                "priced",
                "m",
                "2026-10-01T09:01:00Z",
                AttemptOutcome::Succeeded,
                counts(1, 0, 0, 1),
            ),
            record(
                UsageComponent::Main,
                "free",
                "m",
                "2026-10-01T09:02:00Z",
                AttemptOutcome::Succeeded,
                counts(1, 0, 0, 1),
            ),
            record(
                UsageComponent::Main,
                "free",
                "m",
                "2026-10-01T09:03:00Z",
                AttemptOutcome::Succeeded,
                None,
            ),
        ];
        let cost = |record: &UsageRecord| (record.provider == "priced").then_some(0.25);
        let summary = summarize(&records, GroupBy::Model, cost);
        let priced = &summary.groups[1];
        assert_eq!(priced.key, "priced/m");
        assert_eq!(priced.estimated_cost_usd, Some(0.5));
        assert_eq!(priced.unpriced_calls, 0);
        let free = &summary.groups[0];
        assert_eq!(free.key, "free/m");
        assert_eq!(free.estimated_cost_usd, None);
        assert_eq!(
            free.unpriced_calls, 1,
            "a call without usage isn't unpriced"
        );
        assert_eq!(summary.total.estimated_cost_usd, Some(0.5));
        assert_eq!(summary.total.unpriced_calls, 1);
    }

    #[test]
    fn a_summary_with_no_records_has_no_groups_and_a_null_rate() {
        let summary = summarize(&[], GroupBy::Component, no_price);
        assert!(summary.groups.is_empty());
        assert_eq!(summary.total.calls, 0);
        assert_eq!(summary.total.cache_hit_rate, None);
        assert_eq!(summary.total.estimated_cost_usd, None);
    }

    #[test]
    fn failed_calls_are_counted_with_their_usage() {
        let records = [
            record(
                UsageComponent::Main,
                "a",
                "m",
                "2026-10-01T09:00:00Z",
                AttemptOutcome::Failed,
                counts(4, 0, 0, 0),
            ),
            record(
                UsageComponent::Main,
                "a",
                "m",
                "2026-10-01T09:01:00Z",
                AttemptOutcome::Succeeded,
                counts(4, 0, 0, 2),
            ),
        ];
        let summary = summarize(&records, GroupBy::Component, no_price);
        assert_eq!(summary.total.calls, 2);
        assert_eq!(summary.total.failed_calls, 1);
        assert_eq!(summary.total.uncached_input_tokens, 8);
    }

    #[test]
    fn a_share_is_a_fraction_of_the_whole_and_null_without_one() {
        assert_eq!(share(250, 1000), Some(0.25));
        assert_eq!(share(0, 1000), Some(0.0));
        assert_eq!(share(1000, 1000), Some(1.0));
        assert_eq!(share(0, 0), None);
    }

    #[test]
    fn newest_first_keeps_same_instant_records_latest_written_first() {
        // File order: oldest first, and the two records of 09:00 in the order written.
        let mut records = vec![
            record(
                UsageComponent::Main,
                "a",
                "first",
                "2026-10-01T09:00:00Z",
                AttemptOutcome::Succeeded,
                None,
            ),
            record(
                UsageComponent::Main,
                "a",
                "second",
                "2026-10-01T09:00:00Z",
                AttemptOutcome::Succeeded,
                None,
            ),
            record(
                UsageComponent::Main,
                "a",
                "third",
                "2026-10-01T10:00:00Z",
                AttemptOutcome::Succeeded,
                None,
            ),
        ];
        newest_first(&mut records);
        let models: Vec<&str> = records.iter().map(|r| r.model.as_str()).collect();
        assert_eq!(models, ["third", "second", "first"]);
    }
}
