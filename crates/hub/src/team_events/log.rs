use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::broadcast;

use super::event::{NewTeamEvent, TeamEvent, TeamEventLevel, TeamEventPage};

pub const LOG_CAPACITY: usize = 500;

pub const PROTECTED_ENTRIES: usize = 100;

pub const DEFAULT_PAGE_SIZE: usize = 50;

pub const MAX_PAGE_SIZE: usize = 200;

const NEW_ENTRY_CAPACITY: usize = 256;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PageQuery {
    pub before: Option<u64>,
    pub after: Option<u64>,
    pub limit: Option<usize>,
}

struct Entries {
    next_id: u64,
    list: VecDeque<TeamEvent>,
}

pub struct TeamEventLog {
    boot_id: String,
    entries: Mutex<Entries>,
    new_entries: broadcast::Sender<TeamEvent>,
}

impl TeamEventLog {
    #[must_use]
    pub fn new(boot_id: impl Into<String>) -> Arc<Self> {
        let (new_entries, _first_receiver) = broadcast::channel(NEW_ENTRY_CAPACITY);
        Arc::new(Self {
            boot_id: boot_id.into(),
            entries: Mutex::new(Entries {
                next_id: 1,
                list: VecDeque::with_capacity(LOG_CAPACITY),
            }),
            new_entries,
        })
    }

    #[must_use]
    pub fn boot_id(&self) -> &str {
        &self.boot_id
    }

    pub fn record(&self, new: NewTeamEvent) -> TeamEvent {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        if entries.list.len() >= LOG_CAPACITY {
            let evicted = oldest_unprotected(&entries.list).unwrap_or(0);
            entries.list.remove(evicted);
        }
        let event = TeamEvent {
            id: entries.next_id,
            at: new.at,
            agent: new.agent,
            kind: new.kind,
            level: new.level,
            summary: new.summary,
            target: new.target,
        };
        entries.next_id += 1;
        entries.list.push_back(event.clone());
        self.new_entries.send(event.clone()).ok();
        event
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<TeamEvent> {
        self.new_entries.subscribe()
    }

    #[must_use]
    pub fn page(&self, query: &PageQuery) -> TeamEventPage {
        let limit = query
            .limit
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE);
        let entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        let mut events: Vec<TeamEvent> = entries
            .list
            .iter()
            .rev()
            .filter(|event| query.before.is_none_or(|before| event.id < before))
            .filter(|event| query.after.is_none_or(|after| event.id > after))
            .take(limit + 1)
            .cloned()
            .collect();
        let next_before = if events.len() > limit {
            events.truncate(limit);
            events.last().map(|event| event.id)
        } else {
            None
        };
        TeamEventPage {
            boot_id: self.boot_id.clone(),
            events,
            next_before,
        }
    }
}

fn oldest_unprotected(list: &VecDeque<TeamEvent>) -> Option<usize> {
    let protected_from = list
        .iter()
        .rev()
        .filter(|event| event.level != TeamEventLevel::Info)
        .nth(PROTECTED_ENTRIES - 1)
        .map(|event| event.id);
    list.iter().position(|event| {
        event.level == TeamEventLevel::Info || protected_from.is_some_and(|floor| event.id < floor)
    })
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::team_events::event::TeamEventKind;

    fn entry(level: TeamEventLevel, summary: &str) -> NewTeamEvent {
        NewTeamEvent {
            at: Utc::now(),
            agent: None,
            kind: TeamEventKind::HubNotice,
            level,
            summary: summary.to_string(),
            target: None,
        }
    }

    fn info(summary: &str) -> NewTeamEvent {
        entry(TeamEventLevel::Info, summary)
    }

    fn ids(page: &TeamEventPage) -> Vec<u64> {
        page.events.iter().map(|event| event.id).collect()
    }

    fn filled(count: u64) -> Arc<TeamEventLog> {
        let log = TeamEventLog::new("boot");
        for n in 1..=count {
            log.record(info(&format!("entry {n}")));
        }
        log
    }

    #[test]
    fn ids_increase_from_one_and_every_page_names_the_boot() {
        let log = TeamEventLog::new("boot-7");
        assert_eq!(log.boot_id(), "boot-7");
        let first = log.record(info("first"));
        let second = log.record(info("second"));
        assert_eq!((first.id, second.id), (1, 2));
        let page = log.page(&PageQuery::default());
        assert_eq!(page.boot_id, "boot-7");
        assert_eq!(ids(&page), [2, 1], "newest first");
        assert_eq!(page.next_before, None);
    }

    #[test]
    fn a_full_log_drops_routine_entries_before_failures() {
        let log = TeamEventLog::new("boot");
        for _ in 0..50 {
            log.record(entry(TeamEventLevel::Error, "failed"));
        }
        for n in 0..LOG_CAPACITY {
            log.record(info(&format!("routine {n}")));
        }
        let all = every_entry(&log);
        assert_eq!(all.len(), LOG_CAPACITY);
        let errors = all
            .iter()
            .filter(|event| event.level == TeamEventLevel::Error)
            .count();
        assert_eq!(errors, 50, "every error survives while it is protected");
        assert_eq!(
            all.last().unwrap().id,
            u64::try_from(50 + LOG_CAPACITY).unwrap()
        );
    }

    fn every_entry(log: &TeamEventLog) -> Vec<TeamEvent> {
        let mut all = Vec::new();
        let mut before = None;
        loop {
            let page = log.page(&PageQuery {
                before,
                after: None,
                limit: Some(MAX_PAGE_SIZE),
            });
            all.extend(page.events);
            match page.next_before {
                Some(next) => before = Some(next),
                None => break,
            }
        }
        all.reverse();
        all
    }

    #[test]
    fn eviction_keeps_the_newest_hundred_warnings_and_errors() {
        let log = TeamEventLog::new("boot");
        for n in 0..150 {
            let level = if n % 2 == 0 {
                TeamEventLevel::Warn
            } else {
                TeamEventLevel::Error
            };
            log.record(entry(level, &format!("problem {n}")));
        }
        for n in 0..(2 * LOG_CAPACITY) {
            log.record(info(&format!("routine {n}")));
        }

        let all = every_entry(&log);
        assert_eq!(all.len(), LOG_CAPACITY);
        let problems: Vec<_> = all
            .iter()
            .filter(|event| event.level != TeamEventLevel::Info)
            .map(|event| event.summary.clone())
            .collect();
        let expected: Vec<_> = (50..150).map(|n| format!("problem {n}")).collect();
        assert_eq!(problems, expected, "the newest 100 problems are kept");
        assert!(
            all.windows(2)
                .all(|pair| matches!(pair, [older, newer] if older.id < newer.id)),
            "ids still increase along the log"
        );
    }

    #[test]
    fn a_protected_entry_is_evicted_once_newer_problems_replace_it() {
        let log = TeamEventLog::new("boot");
        let first_problem = log.record(entry(TeamEventLevel::Error, "the first problem"));
        for n in 0..LOG_CAPACITY - 1 {
            log.record(info(&format!("routine {n}")));
        }
        assert!(
            every_entry(&log)
                .iter()
                .any(|event| event.id == first_problem.id),
            "the log is full and the problem is still there"
        );
        for n in 0..PROTECTED_ENTRIES {
            log.record(entry(TeamEventLevel::Warn, &format!("newer {n}")));
        }
        for n in 0..LOG_CAPACITY {
            log.record(info(&format!("more routine {n}")));
        }
        let all = every_entry(&log);
        assert!(
            all.iter().all(|event| event.id != first_problem.id),
            "the first problem aged out behind 100 newer ones"
        );
        let warnings = all
            .iter()
            .filter(|event| event.level == TeamEventLevel::Warn)
            .count();
        assert_eq!(warnings, PROTECTED_ENTRIES);
    }

    #[test]
    fn a_log_of_nothing_but_problems_still_evicts_the_oldest() {
        let log = TeamEventLog::new("boot");
        for n in 0..LOG_CAPACITY + 10 {
            log.record(entry(TeamEventLevel::Error, &format!("problem {n}")));
        }
        let all = every_entry(&log);
        assert_eq!(all.len(), LOG_CAPACITY);
        assert_eq!(all.first().unwrap().id, 11, "the ten oldest were evicted");
    }

    #[test]
    fn pages_walk_back_through_the_log_with_before() {
        let log = filled(120);
        let first = log.page(&PageQuery::default());
        assert_eq!(first.events.len(), DEFAULT_PAGE_SIZE);
        assert_eq!(first.events.first().unwrap().id, 120);
        assert_eq!(first.next_before, Some(71));

        let second = log.page(&PageQuery {
            before: first.next_before,
            ..PageQuery::default()
        });
        assert_eq!(second.events.first().unwrap().id, 70);
        assert_eq!(second.events.last().unwrap().id, 21);
        assert_eq!(second.next_before, Some(21));

        let last = log.page(&PageQuery {
            before: second.next_before,
            ..PageQuery::default()
        });
        assert_eq!(ids(&last), (1..=20).rev().collect::<Vec<_>>());
        assert_eq!(last.next_before, None, "the oldest page ends the walk");
    }

    #[test]
    fn a_page_that_ends_exactly_at_the_oldest_entry_has_no_next() {
        let log = filled(50);
        let page = log.page(&PageQuery::default());
        assert_eq!(page.events.len(), 50);
        assert_eq!(page.next_before, None);
    }

    #[test]
    fn after_returns_only_newer_entries_newest_first() {
        let log = filled(10);
        let page = log.page(&PageQuery {
            after: Some(7),
            ..PageQuery::default()
        });
        assert_eq!(ids(&page), [10, 9, 8]);
        assert_eq!(page.next_before, None);

        let nothing = log.page(&PageQuery {
            after: Some(10),
            ..PageQuery::default()
        });
        assert!(
            nothing.events.is_empty(),
            "expected {:?} to be empty",
            nothing.events
        );
        assert_eq!(nothing.next_before, None);
    }

    #[test]
    fn after_with_more_entries_than_fit_pages_back_to_it_with_before() {
        let log = filled(30);
        let newest = log.page(&PageQuery {
            after: Some(5),
            limit: Some(10),
            ..PageQuery::default()
        });
        assert_eq!(ids(&newest), (21..=30).rev().collect::<Vec<_>>());
        assert_eq!(newest.next_before, Some(21));

        let rest = log.page(&PageQuery {
            before: newest.next_before,
            after: Some(5),
            limit: Some(100),
        });
        assert_eq!(ids(&rest), (6..=20).rev().collect::<Vec<_>>());
        assert_eq!(rest.next_before, None, "the walk stops at the after bound");
    }

    #[test]
    fn a_limit_is_held_between_one_and_the_maximum() {
        let log = filled(300);
        let huge = log.page(&PageQuery {
            limit: Some(10_000),
            ..PageQuery::default()
        });
        assert_eq!(huge.events.len(), MAX_PAGE_SIZE);
        let zero = log.page(&PageQuery {
            limit: Some(0),
            ..PageQuery::default()
        });
        assert_eq!(zero.events.len(), 1);
        assert_eq!(zero.next_before, Some(300));
    }

    #[test]
    fn paging_survives_eviction_gaps() {
        let log = TeamEventLog::new("boot");
        log.record(entry(TeamEventLevel::Error, "kept"));
        for n in 0..LOG_CAPACITY + 20 {
            log.record(info(&format!("routine {n}")));
        }
        let all = every_entry(&log);
        assert_eq!(all.first().unwrap().id, 1);
        assert!(
            matches!(all.as_slice(), [_, second, ..] if second.id == 23),
            "ids 2 to 22 were evicted"
        );
        let around_gap = log.page(&PageQuery {
            before: Some(23),
            limit: Some(5),
            ..PageQuery::default()
        });
        assert_eq!(ids(&around_gap), [1]);
        assert_eq!(around_gap.next_before, None);
    }

    #[tokio::test]
    async fn subscribers_hear_each_new_entry_in_order_and_only_new_ones() {
        let log = TeamEventLog::new("boot");
        log.record(info("before anyone listens"));
        let mut receiver = log.subscribe();
        let a = log.record(info("a"));
        let b = log.record(info("b"));
        assert_eq!(receiver.recv().await.unwrap(), a);
        assert_eq!(receiver.recv().await.unwrap(), b);
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn a_slow_subscriber_is_told_how_many_entries_it_missed() {
        let log = TeamEventLog::new("boot");
        let mut receiver = log.subscribe();
        for n in 0..NEW_ENTRY_CAPACITY + 4 {
            log.record(info(&format!("entry {n}")));
        }
        assert!(matches!(
            receiver.recv().await,
            Err(broadcast::error::RecvError::Lagged(4))
        ));
    }
}
