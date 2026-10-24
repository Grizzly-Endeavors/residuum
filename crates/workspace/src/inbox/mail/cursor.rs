//! The cursor a newest-first list of inbox entries hands out, so a caller can
//! page past the end of a page.

use std::cmp::Ordering;
use std::fmt;

use chrono::DateTime;
use chrono_tz::Tz;

use crate::inbox::InboxRef;

/// Where a page of a newest-first list stopped: the time and the ref of its
/// last entry, written `<unix milliseconds>:<ref>`. A ref may hold `:`, so the
/// ref is everything after the first one.
///
/// Cursors compare in the order the lists use: newest first, then by ref
/// ascending. The entries a cursor leaves for the next page are the ones that
/// come after it, which is [`Self::precedes`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailCursor {
    at_ms: i64,
    item: InboxRef,
}

/// The order of every list a cursor pages: the later time first, a tie going
/// to the lower ref.
fn list_order(a_ms: i64, a: &InboxRef, b_ms: i64, b: &InboxRef) -> Ordering {
    b_ms.cmp(&a_ms).then_with(|| a.cmp(b))
}

impl MailCursor {
    /// The cursor of the entry written at `at` and named by `item`: a thread
    /// by its latest time and thread id, a single message by its own.
    #[must_use]
    pub fn new(at: DateTime<Tz>, item: InboxRef) -> Self {
        Self {
            at_ms: at.timestamp_millis(),
            item,
        }
    }

    /// The cursor a [`Display`](fmt::Display) rendering wrote, or `None` when
    /// `raw` is not one.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let (at, item) = raw.split_once(':')?;
        Some(Self {
            at_ms: at.parse().ok()?,
            item: InboxRef::parse(item).ok()?,
        })
    }

    /// Whether the entry written at `at` and named by `item` comes after this
    /// cursor in the list's order.
    #[must_use]
    pub fn precedes(&self, at: DateTime<Tz>, item: &InboxRef) -> bool {
        list_order(self.at_ms, &self.item, at.timestamp_millis(), item) == Ordering::Less
    }
}

impl fmt::Display for MailCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.at_ms, self.item)
    }
}

impl Ord for MailCursor {
    fn cmp(&self, other: &Self) -> Ordering {
        list_order(self.at_ms, &self.item, other.at_ms, &other.item)
    }
}

impl PartialOrd for MailCursor {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> DateTime<Tz> {
        DateTime::from_timestamp(secs, 0)
            .unwrap()
            .with_timezone(&chrono_tz::UTC)
    }

    fn item(raw: &str) -> InboxRef {
        InboxRef::parse(raw).unwrap()
    }

    #[test]
    fn a_cursor_survives_a_round_trip_even_when_the_id_holds_colons() {
        let cursor = MailCursor::new(at(1_790_000_000), item("scout/user/20260930_a:b:c"));
        let raw = cursor.to_string();
        assert_eq!(raw, "1790000000000:scout/user/20260930_a:b:c");

        let parsed = MailCursor::parse(&raw).unwrap();
        assert_eq!(parsed.at_ms, 1_790_000_000_000);
        assert_eq!(parsed.item.to_string(), "scout/user/20260930_a:b:c");
        assert_eq!(parsed, cursor);
        assert_eq!(parsed.to_string(), raw);
    }

    #[test]
    fn a_cursor_needs_a_time_and_a_ref() {
        for raw in [
            "",
            "scout/user/id",
            "soon:scout/user/id",
            "1790000000:",
            "1790000000:scout",
            "1790000000",
        ] {
            assert_eq!(MailCursor::parse(raw), None, "{raw:?} isn't a cursor");
        }
    }

    #[test]
    fn cursors_order_newest_first_and_a_tie_by_ref() {
        let newest = MailCursor::new(at(300), item("b/user/x"));
        let tie_low = MailCursor::new(at(200), item("a/user/x"));
        let tie_high = MailCursor::new(at(200), item("b/user/x"));
        let oldest = MailCursor::new(at(100), item("a/user/x"));

        let mut shuffled = vec![
            oldest.clone(),
            tie_high.clone(),
            newest.clone(),
            tie_low.clone(),
        ];
        shuffled.sort();
        assert_eq!(shuffled, [newest, tie_low, tie_high, oldest]);
    }

    #[test]
    fn an_entry_comes_after_a_cursor_when_it_is_older_or_ties_with_a_higher_ref() {
        let cursor = MailCursor::new(at(200), item("b/user/x"));

        assert!(cursor.precedes(at(100), &item("a/user/x")), "older");
        assert!(
            cursor.precedes(at(200), &item("c/user/x")),
            "tie, higher ref"
        );
        assert!(
            !cursor.precedes(at(200), &item("b/user/x")),
            "the cursor's own entry"
        );
        assert!(
            !cursor.precedes(at(200), &item("a/user/x")),
            "tie, lower ref"
        );
        assert!(!cursor.precedes(at(300), &item("z/user/x")), "newer");
    }
}
