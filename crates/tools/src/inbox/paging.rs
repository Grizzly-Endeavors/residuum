//! Paging and text search for the inbox listing tools: `limit`, `before` and
//! `query` mean the same in `inbox_list` and `user_inbox_list`.
//!
//! A list is newest first, ties by ref. A page ends with a cursor
//! ([`MailCursor`], the one the hub's thread list hands out) that the next call
//! passes back as `before`.

use serde_json::Value;

use residuum_inference::ToolError;
use residuum_workspace::inbox::mail::MailCursor;

/// How many items a call lists when it names no `limit`.
pub(super) const DEFAULT_LIMIT: usize = 50;

/// The most items a call lists; a larger `limit` counts as this.
pub(super) const MAX_LIMIT: usize = 200;

/// Which page of a list a call asks for.
pub(super) struct PageRequest {
    limit: usize,
    before: Option<MailCursor>,
    query: Option<String>,
}

/// The entries of one page, and what the list holds beyond it.
pub(super) struct Page<T> {
    /// This page's entries, newest first.
    pub items: Vec<T>,
    /// How many entries the list holds from the page's start, the page's own
    /// and those on later pages.
    pub total: usize,
    /// The cursor of the page's last entry when later pages follow.
    pub next: Option<MailCursor>,
}

impl PageRequest {
    /// Read `limit`, `before` and `query` from a call's arguments.
    ///
    /// # Errors
    /// [`ToolError::InvalidArguments`] when `limit` is not a whole number of
    /// at least 1, or `before` is not a cursor a listing returned.
    pub(super) fn parse(arguments: &Value) -> Result<Self, ToolError> {
        Ok(Self {
            limit: parse_limit(arguments.get("limit"))?,
            before: parse_before(arguments.get("before"))?,
            query: arguments
                .get("query")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|query| !query.is_empty())
                .map(str::to_string),
        })
    }

    /// The text entries must hold, when the call named some.
    pub(super) fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }

    /// The page `entries` make: sorted, cut to the entries after the `before`
    /// cursor, and limited. Each entry comes with its own cursor.
    pub(super) fn page<T>(&self, mut entries: Vec<(MailCursor, T)>) -> Page<T> {
        entries.sort_by(|(a, _), (b, _)| a.cmp(b));
        if let Some(before) = &self.before {
            entries.retain(|(cursor, _)| before < cursor);
        }
        let total = entries.len();
        entries.truncate(self.limit);
        let next = entries
            .last()
            .filter(|_| total > self.limit)
            .map(|(cursor, _)| cursor.clone());
        Page {
            items: entries.into_iter().map(|(_, entry)| entry).collect(),
            total,
            next,
        }
    }

    /// The line that opens a page holding `count` items of `kind` ("inbox",
    /// "archived" or "user inbox"), counted from the page's start.
    pub(super) fn header(&self, count: usize, kind: &str) -> String {
        let mut header = format!("{count} {kind} item(s)");
        if self.query.is_some() {
            header.push_str(" match");
        }
        if self.before.is_some() {
            header.push_str(" after the cursor");
        }
        header.push_str(", newest first:");
        header
    }

    /// The answer when no items of `kind` are left to list.
    pub(super) fn empty_message(&self, kind: &str) -> String {
        let mut message = format!("No {kind} items");
        match &self.query {
            Some(query) => {
                message.push_str(" match \"");
                message.push_str(query);
                message.push('"');
            }
            None => message.push_str(" found"),
        }
        if self.before.is_some() {
            message.push_str(" after the cursor");
        }
        message.push('.');
        message
    }
}

/// The line that ends a page that later pages follow: how much was shown and
/// the call that shows the next page. `filters` names the parameters the call
/// repeats.
pub(super) fn more_note(shown: usize, total: usize, next: &MailCursor, filters: &str) -> String {
    format!(
        "Showing {shown} of {total}. For the next page, call again with before=\"{next}\" and the same {filters}."
    )
}

fn parse_limit(value: Option<&Value>) -> Result<usize, ToolError> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(DEFAULT_LIMIT);
    };
    let whole = match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.trim().parse::<u64>().ok(),
        Value::Null | Value::Bool(_) | Value::Array(_) | Value::Object(_) => None,
    };
    match whole {
        None => Err(ToolError::InvalidArguments(format!(
            "limit must be a whole number, not {value}"
        ))),
        Some(0) => Err(ToolError::InvalidArguments(
            "limit must be at least 1".to_string(),
        )),
        Some(limit) => Ok(usize::try_from(limit).unwrap_or(usize::MAX).min(MAX_LIMIT)),
    }
}

fn parse_before(value: Option<&Value>) -> Result<Option<MailCursor>, ToolError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(raw)) if raw.trim().is_empty() => Ok(None),
        Some(Value::String(raw)) => MailCursor::parse(raw.trim())
            .map(Some)
            .ok_or_else(foreign_cursor),
        Some(Value::Bool(_) | Value::Number(_) | Value::Array(_) | Value::Object(_)) => {
            Err(foreign_cursor())
        }
    }
}

fn foreign_cursor() -> ToolError {
    ToolError::InvalidArguments(
        "the before cursor isn't one this list returned. Pass the cursor the previous page ended with, or leave before out to start from the newest items".to_string(),
    )
}

/// The `limit` property of a listing tool's parameters.
pub(super) fn limit_schema() -> Value {
    serde_json::json!({
        "type": "integer",
        "description": format!(
            "How many items one call lists (default {DEFAULT_LIMIT}, at most {MAX_LIMIT}; a larger value counts as {MAX_LIMIT})."
        )
    })
}

/// The `before` property of a listing tool's parameters.
pub(super) fn before_schema() -> Value {
    serde_json::json!({
        "type": "string",
        "description": "The cursor a previous call ended with (its \"call again with before=...\" line). Lists the items after it, which are older. Leave out to start from the newest."
    })
}

/// The `query` property of a listing tool's parameters.
pub(super) fn query_schema() -> Value {
    serde_json::json!({
        "type": "string",
        "description": "Only items whose title, body or attachment filenames contain this text, ignoring case. Ignored when blank."
    })
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use residuum_workspace::inbox::InboxRef;

    use super::*;

    fn request(arguments: &Value) -> PageRequest {
        PageRequest::parse(arguments).unwrap()
    }

    fn refusal(arguments: &Value) -> String {
        match PageRequest::parse(arguments) {
            Err(ToolError::InvalidArguments(message)) => message,
            other => panic!("the arguments are refused: {:?}", other.map(|_| ())),
        }
    }

    fn entry(secs: i64, id: &str) -> (MailCursor, String) {
        let at = DateTime::from_timestamp(secs, 0)
            .unwrap()
            .with_timezone(&chrono_tz::UTC);
        let item = InboxRef::parse(&format!("atlas/user/{id}")).unwrap();
        (MailCursor::new(at, item), id.to_string())
    }

    #[test]
    fn no_arguments_ask_for_the_first_default_sized_page() {
        let page = request(&serde_json::json!({}));
        assert_eq!(page.limit, DEFAULT_LIMIT);
        assert!(page.before.is_none());
        assert_eq!(page.query(), None);
    }

    #[test]
    fn a_limit_above_the_maximum_counts_as_the_maximum() {
        assert_eq!(
            request(&serde_json::json!({"limit": 5_000})).limit,
            MAX_LIMIT
        );
        assert_eq!(request(&serde_json::json!({"limit": 7})).limit, 7);
        assert_eq!(
            request(&serde_json::json!({"limit": "12"})).limit,
            12,
            "a number written as text counts"
        );
        assert_eq!(request(&serde_json::json!({"limit": null})).limit, 50);
    }

    #[test]
    fn a_limit_that_is_not_a_whole_number_of_at_least_one_is_refused() {
        assert_eq!(
            refusal(&serde_json::json!({"limit": 0})),
            "limit must be at least 1"
        );
        assert_eq!(
            refusal(&serde_json::json!({"limit": "0"})),
            "limit must be at least 1"
        );
        for bad in [
            serde_json::json!(-3),
            serde_json::json!(2.5),
            serde_json::json!("many"),
            serde_json::json!(true),
            serde_json::json!([5]),
        ] {
            let message = refusal(&serde_json::json!({ "limit": bad }));
            assert!(
                message.starts_with("limit must be a whole number, not "),
                "{message}"
            );
        }
    }

    #[test]
    fn a_before_that_is_not_a_returned_cursor_is_refused() {
        for bad in [
            serde_json::json!("nonsense"),
            serde_json::json!("soon:atlas/user/x"),
            serde_json::json!(17),
        ] {
            let message = refusal(&serde_json::json!({ "before": bad }));
            assert!(
                message.contains("the before cursor isn't one this list returned"),
                "{message}"
            );
        }
    }

    #[test]
    fn a_blank_before_or_query_is_left_out() {
        let page = request(&serde_json::json!({"before": "  ", "query": "   "}));
        assert!(page.before.is_none());
        assert_eq!(page.query(), None);
        assert_eq!(
            request(&serde_json::json!({"query": "  Budget "})).query(),
            Some("Budget")
        );
    }

    #[test]
    fn a_page_is_cut_after_the_cursor_and_ends_with_the_cursor_of_its_last_entry() {
        let entries = || {
            vec![
                entry(100, "c"),
                entry(300, "a"),
                entry(200, "b"),
                entry(200, "a2"),
            ]
        };

        let first = request(&serde_json::json!({"limit": 2})).page(entries());
        assert_eq!(first.items, ["a", "a2"]);
        assert_eq!(first.total, 4);

        let before = first.next.unwrap().to_string();
        let second = request(&serde_json::json!({"limit": 2, "before": before})).page(entries());
        assert_eq!(second.items, ["b", "c"]);
        assert_eq!(second.total, 2, "counted from the page's start");
        assert_eq!(second.next, None);
    }

    #[test]
    fn a_page_that_holds_the_rest_has_no_next_cursor() {
        let page =
            request(&serde_json::json!({"limit": 2})).page(vec![entry(1, "a"), entry(2, "b")]);
        assert_eq!(page.items, ["b", "a"]);
        assert_eq!(page.next, None);
    }

    #[test]
    fn headers_and_empty_answers_say_what_narrowed_the_list() {
        let plain = request(&serde_json::json!({}));
        assert_eq!(
            plain.header(3, "user inbox"),
            "3 user inbox item(s), newest first:"
        );
        assert_eq!(plain.empty_message("archived"), "No archived items found.");

        let cursor = entry(1, "a").0.to_string();
        let narrowed = request(&serde_json::json!({"query": "budget", "before": cursor}));
        assert_eq!(
            narrowed.header(3, "inbox"),
            "3 inbox item(s) match after the cursor, newest first:"
        );
        assert_eq!(
            narrowed.empty_message("inbox"),
            "No inbox items match \"budget\" after the cursor."
        );
    }
}
